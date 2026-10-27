//! Starter og taler med sidecaren (én proces pr. account) via JSON-linjer.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum Event {
    Status { state: String, detail: Option<String> },
    Chat(String),
    Log(String),
    Error(String),
    MsaCode { code: String, url: String, expires_in: u64 },
    TokenCache { name: String, data: Value },
    Progress { done: u64, total: u64 },
    Exited,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Sidecar {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    child: tokio::sync::Mutex<Child>,
    pending: Pending,
    next_id: AtomicU64,
}

pub fn exe_path() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    dir.join(format!("cellkeeper-sidecar{}", std::env::consts::EXE_SUFFIX))
}

impl Sidecar {
    pub async fn spawn(events: mpsc::UnboundedSender<Event>) -> Result<Arc<Sidecar>> {
        let path = exe_path();
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: intet konsolvindue

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("Kunne ikke starte hjælpeprogrammet ({}): {e}", path.display()))?;
        #[cfg(windows)]
        job::assign(&child);

        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Ingen stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("Ingen stderr"))?;
        let stdin = child.stdin.take();
        let pending: Pending = Arc::default();

        let reader_pending = pending.clone();
        let reader_events = events.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                handle_line(&line, &reader_pending, &reader_events);
            }
            for (_, tx) in reader_pending.lock().unwrap().drain() {
                let _ = tx.send(Err("Hjælpeprogrammet stoppede uventet".into()));
            }
            let _ = reader_events.send(Event::Exited);
        });

        // stderr bruges kun til fejlsøgning; tokens skrives aldrig dertil.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line: String = line.chars().take(300).collect();
                if line.to_lowercase().contains("error") {
                    let _ = events.send(Event::Log(format!("(intern) {line}")));
                }
            }
        });

        Ok(Arc::new(Sidecar {
            stdin: tokio::sync::Mutex::new(stdin),
            child: tokio::sync::Mutex::new(child),
            pending,
            next_id: AtomicU64::new(1),
        }))
    }

    pub async fn request(&self, cmd: &str, mut args: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        if let Value::Object(map) = &mut args {
            map.insert("id".into(), json!(id));
            map.insert("cmd".into(), json!(cmd));
        }
        let mut line = serde_json::to_string(&args)?;
        line.push('\n');
        {
            let mut guard = self.stdin.lock().await;
            let stdin = guard.as_mut().ok_or_else(|| anyhow!("Hjælpeprogrammet er lukket"))?;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!(e)),
            Ok(Err(_)) => Err(anyhow!("Hjælpeprogrammet stoppede uventet")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("Timeout efter {} sek ({cmd})", timeout.as_secs()))
            }
        }
    }

    /// Logger ud, lukker stdin og dræber processen, hvis den ikke selv stopper.
    pub async fn shutdown(&self) {
        let _ = self.request("logout", json!({}), Duration::from_secs(5)).await;
        self.stdin.lock().await.take();
        let mut child = self.child.lock().await;
        if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
            let _ = child.kill().await;
        }
    }

    pub async fn kill(&self) {
        self.stdin.lock().await.take();
        let _ = self.child.lock().await.kill().await;
    }
}

fn handle_line(line: &str, pending: &Pending, events: &mpsc::UnboundedSender<Event>) {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let s = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let event = match msg.get("type").and_then(Value::as_str).unwrap_or_default() {
        "result" => {
            let id = msg.get("id").and_then(Value::as_u64).unwrap_or(0);
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                let res = if msg.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(msg.get("data").cloned().unwrap_or(Value::Null))
                } else {
                    Err(s("error"))
                };
                let _ = tx.send(res);
            }
            return;
        }
        "status" => Event::Status {
            state: s("state"),
            detail: msg.get("reason").or(msg.get("username")).and_then(Value::as_str).map(String::from),
        },
        "chat" => Event::Chat(s("text")),
        "log" => Event::Log(s("message")),
        "error" => Event::Error(s("message")),
        "msa_code" => Event::MsaCode {
            code: s("code"),
            url: s("url"),
            expires_in: msg.get("expiresIn").and_then(Value::as_u64).unwrap_or(900),
        },
        "token_cache" => Event::TokenCache {
            name: s("name"),
            data: msg.get("data").cloned().unwrap_or(Value::Null),
        },
        "progress" => Event::Progress {
            done: msg.get("done").and_then(Value::as_u64).unwrap_or(0),
            total: msg.get("total").and_then(Value::as_u64).unwrap_or(0),
        },
        _ => return,
    };
    let _ = events.send(event);
}

/// Windows: alle sidecars lægges i et Job Object med "kill on close", så de
/// dør sammen med appen – også hvis appen crasher.
#[cfg(windows)]
mod job {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn handle() -> Option<HANDLE> {
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            );
            Some(Job(h))
        })
        .as_ref()
        .map(|j| j.0)
    }

    pub fn assign(child: &tokio::process::Child) {
        if let (Some(job), Some(raw)) = (handle(), child.raw_handle()) {
            unsafe {
                AssignProcessToJobObject(job, raw as HANDLE);
            }
        }
    }
}
