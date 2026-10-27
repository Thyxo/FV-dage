//! Appens samlede tilstand: data, krypteret lager, seneste kørsel og events til UI.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::cellmanager::Manager;
use crate::model::{new_id, Account, Cell, Data, Settings};
use crate::run::{LogLine, Run, RunItem, RunState};
use crate::sidecar::Sidecar;
use crate::vault::{write_atomic, Vault, VaultStore};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerStatus {
    pub ok: Option<bool>,
    pub message: String,
    pub last_sync: Option<DateTime<Utc>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub has_tokens: bool,
    pub cell_count: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellView {
    #[serde(flatten)]
    pub cell: Cell,
    pub account_id: Option<String>,
    /// Hvorfor cellen ikke kan fornys lige nu (ellers `None`).
    pub problem: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub accounts: Vec<AccountView>,
    pub cells: Vec<CellView>,
    pub settings: Settings,
    pub api_key_set: bool,
    pub manager: ManagerStatus,
    pub run: Option<Run>,
    pub run_active: bool,
    pub notice: Option<String>,
    pub data_dir: String,
}

pub struct App {
    pub handle: AppHandle,
    pub dir: PathBuf,
    pub data: Mutex<Data>,
    pub vault: Mutex<Vault>,
    vault_store: VaultStore,
    pub run: Mutex<Option<Run>>,
    pub run_active: AtomicBool,
    pub cancel: AtomicBool,
    /// Sidecar til et igangværende Microsoft-login (så det kan annulleres).
    pub login: tokio::sync::Mutex<Option<Arc<Sidecar>>>,
    pub notice: Mutex<Option<String>>,
    pub manager_status: Mutex<ManagerStatus>,
}

impl App {
    pub fn load(handle: AppHandle, dir: PathBuf) -> Result<App> {
        fs::create_dir_all(dir.join("runs"))?;
        let state_path = dir.join("state.json");
        let data: Data = fs::read(&state_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let (vault_store, vault, warning) = VaultStore::open(&dir)?;
        let run = load_last_run(&dir);
        Ok(App {
            handle,
            dir,
            data: Mutex::new(data),
            vault: Mutex::new(vault),
            vault_store,
            run: Mutex::new(run),
            run_active: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            login: tokio::sync::Mutex::new(None),
            notice: Mutex::new(warning),
            manager_status: Mutex::new(ManagerStatus::default()),
        })
    }

    pub fn save_data(&self) {
        let bytes = serde_json::to_vec_pretty(&*self.data.lock().unwrap()).unwrap_or_default();
        if let Err(e) = write_atomic(&self.dir.join("state.json"), &bytes) {
            self.set_notice(format!("Kunne ikke gemme data: {e}"));
        }
    }

    pub fn save_vault(&self) {
        let res = self.vault_store.save(&self.vault.lock().unwrap());
        if let Err(e) = res {
            self.set_notice(format!("Kunne ikke gemme logins: {e}"));
        }
    }

    pub fn set_notice(&self, msg: String) {
        *self.notice.lock().unwrap() = Some(msg);
    }

    pub fn settings(&self) -> Settings {
        self.data.lock().unwrap().settings.clone()
    }

    pub fn manager(&self) -> Result<Manager> {
        let url = self.settings().manager_url;
        let key = self.vault.lock().unwrap().api_key.clone();
        Manager::new(&url, key)
    }

    pub fn view(&self) -> View {
        let data = self.data.lock().unwrap();
        let vault = self.vault.lock().unwrap();
        let has_tokens = |id: &str| vault.tokens.get(id).is_some_and(|t| !t.is_empty());

        let accounts = data
            .accounts
            .iter()
            .map(|a| AccountView {
                account: a.clone(),
                has_tokens: has_tokens(&a.id),
                cell_count: data
                    .cells
                    .iter()
                    .filter(|c| c.account_name.eq_ignore_ascii_case(&a.name))
                    .count(),
            })
            .collect();

        let cells = data
            .cells
            .iter()
            .map(|c| {
                let acc = data.account_for_cell(c);
                let problem = if c.removed_remote {
                    Some("Slettet i cell-manager".to_string())
                } else {
                    match acc {
                        None => Some(format!("Ingen account der hedder {}", c.account_name)),
                        Some(a) if !a.enabled => Some("Account er deaktiveret".to_string()),
                        Some(a) if !has_tokens(&a.id) => Some("Account mangler login".to_string()),
                        _ => None,
                    }
                };
                CellView { cell: c.clone(), account_id: acc.map(|a| a.id.clone()), problem }
            })
            .collect();

        View {
            accounts,
            cells,
            settings: data.settings.clone(),
            api_key_set: vault.api_key.as_ref().is_some_and(|k| !k.is_empty()),
            manager: self.manager_status.lock().unwrap().clone(),
            run: self.run.lock().unwrap().as_ref().map(Run::without_logs),
            run_active: self.run_active.load(Ordering::SeqCst),
            notice: self.notice.lock().unwrap().clone(),
            data_dir: self.dir.display().to_string(),
        }
    }

    pub fn emit_view(&self) {
        let _ = self.handle.emit("view", self.view());
    }

    // ── Kørsel ───────────────────────────────────────────────────────────

    pub fn save_run(&self) {
        let run = self.run.lock().unwrap().clone();
        if let Some(run) = run {
            if let Ok(bytes) = serde_json::to_vec(&run) {
                let _ = write_atomic(&self.dir.join("runs").join(format!("{}.json", run.id)), &bytes);
            }
            let _ = self.handle.emit("run", run.without_logs());
        }
    }

    /// Ændrer en kørsels-linje, gemmer journalen og giver UI besked.
    pub fn update_item(&self, cell_id: &str, f: impl FnOnce(&mut RunItem)) {
        {
            let mut guard = self.run.lock().unwrap();
            let Some(run) = guard.as_mut() else { return };
            let Some(item) = run.items.iter_mut().find(|i| i.cell_id == cell_id) else { return };
            f(item);
        }
        self.save_run();
    }

    pub fn item(&self, cell_id: &str) -> Option<RunItem> {
        let guard = self.run.lock().unwrap();
        guard.as_ref()?.items.iter().find(|i| i.cell_id == cell_id).cloned()
    }

    pub fn log(&self, cell_id: &str, level: &str, text: impl Into<String>) {
        let line = LogLine::now(level, text.into());
        let mut cell_name = String::new();
        {
            let mut guard = self.run.lock().unwrap();
            if let Some(item) = guard.as_mut().and_then(|r| r.items.iter_mut().find(|i| i.cell_id == cell_id)) {
                cell_name = item.cell_name.clone();
                item.log.push(line.clone());
                if item.log.len() > 400 {
                    item.log.drain(..100);
                }
            }
        }
        let _ = self.handle.emit(
            "log",
            serde_json::json!({ "cellId": cell_id, "cellName": cell_name, "line": line }),
        );
    }

    // ── cell-manager ─────────────────────────────────────────────────────

    /// Henter celler fra cell-manager og fletter dem ind. cell-manager ejer
    /// navn, account, rank, block og dage – lokale felter bevares.
    pub async fn sync_cells(&self) -> Result<()> {
        let result = async { self.manager()?.cells().await }.await;
        let remote = match result {
            Ok(r) => r,
            Err(e) => {
                {
                    let mut status = self.manager_status.lock().unwrap();
                    status.ok = Some(false);
                    status.message = e.to_string();
                }
                self.emit_view();
                return Err(e);
            }
        };

        {
            let mut data = self.data.lock().unwrap();
            for c in data.cells.iter_mut() {
                if c.remote_id.is_some() && !remote.iter().any(|r| Some(&r.id) == c.remote_id.as_ref()) {
                    c.removed_remote = true;
                }
            }
            for r in &remote {
                let idx = data
                    .cells
                    .iter()
                    .position(|c| c.remote_id.as_deref() == Some(r.id.as_str()))
                    .or_else(|| {
                        data.cells
                            .iter()
                            .position(|c| c.remote_id.is_none() && c.cell_name.eq_ignore_ascii_case(&r.cell_name))
                    });
                let days = Some(r.days_left.clamp(0, 9) as u8);
                match idx {
                    Some(i) => {
                        let c = &mut data.cells[i];
                        c.remote_id = Some(r.id.clone());
                        c.cell_name = r.cell_name.clone();
                        c.account_name = r.account_name.clone();
                        c.rank = r.rank.clone();
                        c.block = r.block.clone();
                        c.days_left = days;
                        c.removed_remote = false;
                    }
                    None => data.cells.push(Cell {
                        id: new_id(),
                        cell_name: r.cell_name.clone(),
                        account_name: r.account_name.clone(),
                        remote_id: Some(r.id.clone()),
                        rank: r.rank.clone(),
                        block: r.block.clone(),
                        days_left: days,
                        removed_remote: false,
                        last_renewed: None,
                        last_info: None,
                        last_info_at: None,
                    }),
                }
            }
            data.cells.sort_by(|a, b| {
                a.days_left.unwrap_or(0).cmp(&b.days_left.unwrap_or(0)).then(a.cell_name.cmp(&b.cell_name))
            });
        }
        self.save_data();
        *self.manager_status.lock().unwrap() = ManagerStatus {
            ok: Some(true),
            message: format!("{} celler hentet", remote.len()),
            last_sync: Some(Utc::now()),
        };
        self.emit_view();
        Ok(())
    }
}

/// Indlæser seneste kørsel. Var den i gang, da appen lukkede, markeres den
/// som afbrudt, så brugeren kan genoptage de manglende celler.
fn load_last_run(dir: &std::path::Path) -> Option<Run> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir.join("runs"))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    // Behold kun de 20 nyeste journaler.
    if files.len() > 20 {
        for old in files.drain(..files.len() - 20) {
            let _ = fs::remove_file(old);
        }
    }
    let mut run: Run = serde_json::from_slice(&fs::read(files.last()?).ok()?).ok()?;
    if run.state == RunState::Running {
        run.mark_interrupted();
    }
    Some(run)
}
