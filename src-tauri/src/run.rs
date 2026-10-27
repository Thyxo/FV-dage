//! "Tilføj dage": kører fornyelses-makroen for hver celle med dens account.
//!
//! Hver celle får sin egen sidecar-proces. Trin og resultater skrives løbende
//! til `runs/<id>.json`, så en afbrudt kørsel kan genoptages pr. celle.

use chrono::{DateTime, Local, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::{mpsc, Semaphore};

use crate::app::App;
use crate::ceinfo::{self, CeInfo};
use crate::macro_lang::{self, Step, Validation, Vars};
use crate::model::Settings;
use crate::sidecar::{Event, Sidecar};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    pub time: String,
    pub level: String,
    pub text: String,
}

impl LogLine {
    pub fn now(level: &str, text: String) -> LogLine {
        LogLine { time: Local::now().format("%H:%M:%S").to_string(), level: level.into(), text }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    Done,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Pending,
    Running,
    /// Verificeret fuld.
    Done,
    /// Gennemført, men ikke fuld eller ikke verificeret.
    Warning,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    None,
    Done,
    Failed,
    /// Brugeren skal bekræfte, før cell-manager opdateres.
    NeedsConfirm,
    /// Cellen findes kun lokalt.
    NotLinked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunItem {
    pub cell_id: String,
    pub cell_name: String,
    pub account_id: String,
    pub account_name: String,
    pub state: ItemState,
    pub step: String,
    #[serde(default)]
    pub progress: Option<(u64, u64)>,
    pub attempts: u8,
    pub clicks: u32,
    pub before: Option<CeInfo>,
    pub after: Option<CeInfo>,
    pub error: Option<String>,
    pub sync: SyncState,
    #[serde(default)]
    pub sync_error: Option<String>,
    #[serde(default)]
    pub log: Vec<LogLine>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub state: RunState,
    pub items: Vec<RunItem>,
}

impl Run {
    pub fn without_logs(&self) -> Run {
        let mut r = self.clone();
        for i in r.items.iter_mut() {
            i.log.clear();
        }
        r
    }

    pub fn mark_interrupted(&mut self) {
        self.state = RunState::Interrupted;
        for i in self.items.iter_mut().filter(|i| i.state == ItemState::Running) {
            i.state = ItemState::Failed;
            i.error = Some("Afbrudt: appen lukkede under kørslen".into());
        }
    }
}

pub fn new_item(cell_id: &str, cell_name: &str, account_id: &str, account_name: &str) -> RunItem {
    RunItem {
        cell_id: cell_id.into(),
        cell_name: cell_name.into(),
        account_id: account_id.into(),
        account_name: account_name.into(),
        state: ItemState::Pending,
        step: "Venter".into(),
        progress: None,
        attempts: 0,
        clicks: 0,
        before: None,
        after: None,
        error: None,
        sync: SyncState::None,
        sync_error: None,
        log: vec![],
    }
}

pub fn new_run(items: Vec<RunItem>) -> Run {
    Run {
        id: Local::now().format("%Y%m%d-%H%M%S").to_string(),
        started_at: Utc::now(),
        finished_at: None,
        state: RunState::Running,
        items,
    }
}

/// Kører de angivne celler i den aktuelle kørsel. Kaldes også ved genforsøg.
pub async fn execute(app: Arc<App>, cell_ids: Vec<String>) {
    let settings = app.settings();
    app.cancel.store(false, Ordering::SeqCst);
    {
        let mut guard = app.run.lock().unwrap();
        if let Some(run) = guard.as_mut() {
            run.state = RunState::Running;
            run.finished_at = None;
            // Nyt forsøg starter rent (også ved "Prøv igen" og "Genoptag").
            for i in run.items.iter_mut().filter(|i| cell_ids.contains(&i.cell_id)) {
                i.state = ItemState::Pending;
                i.step = "Venter".into();
                i.error = None;
                i.before = None;
                i.after = None;
                i.clicks = 0;
                i.attempts = 0;
                i.progress = None;
                i.sync = SyncState::None;
                i.sync_error = None;
            }
        }
    }
    app.save_run();
    app.emit_view();

    let sem = Arc::new(Semaphore::new(usize::from(settings.max_parallel.clamp(1, 8))));
    let mut tasks = Vec::new();
    for (n, cell_id) in cell_ids.into_iter().enumerate() {
        let Ok(permit) = sem.clone().acquire_owned().await else { break };
        if app.cancel.load(Ordering::SeqCst) {
            app.update_item(&cell_id, |i| {
                i.state = ItemState::Cancelled;
                i.step = "Annulleret".into();
            });
            continue;
        }
        if n > 0 {
            // Spred logins ud, så det ikke ligner spam.
            let jitter = rand::thread_rng().gen_range(0..=settings.login_jitter_ms);
            tokio::time::sleep(Duration::from_millis(settings.login_gap_ms + jitter)).await;
        }
        let app2 = app.clone();
        let settings = settings.clone();
        tasks.push(tokio::spawn(async move {
            process_item(&app2, &cell_id, &settings).await;
            drop(permit);
        }));
    }
    for t in tasks {
        let _ = t.await;
    }

    {
        let mut guard = app.run.lock().unwrap();
        if let Some(run) = guard.as_mut() {
            run.finished_at = Some(Utc::now());
            run.state = if app.cancel.load(Ordering::SeqCst) { RunState::Cancelled } else { RunState::Done };
        }
    }
    app.run_active.store(false, Ordering::SeqCst);
    app.save_run();
    app.save_data();
    let _ = app.sync_cells().await;
    app.emit_view();
}

enum Fail {
    /// Forbindelsesfejl: kan prøves igen automatisk.
    Retry(String),
    Stop(String),
}

impl Fail {
    fn msg(&self) -> &str {
        match self {
            Fail::Retry(m) | Fail::Stop(m) => m,
        }
    }
}

async fn process_item(app: &Arc<App>, cell_id: &str, settings: &Settings) {
    let max_attempts = settings.retries.saturating_add(1);
    for attempt in 1..=max_attempts {
        app.update_item(cell_id, |i| {
            i.state = ItemState::Running;
            i.after = None;
            i.attempts = i.attempts.saturating_add(1);
            i.error = None;
            i.step = "Starter".into();
            i.progress = None;
        });
        match attempt_item(app, cell_id, settings).await {
            Ok(()) => {
                finish_item(app, cell_id, settings).await;
                return;
            }
            Err(_) if app.cancel.load(Ordering::SeqCst) => {
                app.update_item(cell_id, |i| {
                    i.state = ItemState::Cancelled;
                    i.step = "Annulleret".into();
                });
                return;
            }
            Err(Fail::Retry(msg)) if attempt < max_attempts => {
                app.log(cell_id, "warn", format!("{msg} – prøver igen om 15 sek"));
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
            Err(fail) => {
                let msg = fail.msg().to_string();
                app.log(cell_id, "error", msg.clone());
                app.update_item(cell_id, |i| {
                    i.state = ItemState::Failed;
                    i.step = "Fejlede".into();
                    i.error = Some(msg.clone());
                });
                set_account_error(app, cell_id, Some(msg));
                return;
            }
        }
    }
}

fn set_account_error(app: &App, cell_id: &str, error: Option<String>) {
    let Some(item) = app.item(cell_id) else { return };
    let mut data = app.data.lock().unwrap();
    if let Some(a) = data.accounts.iter_mut().find(|a| a.id == item.account_id) {
        a.last_error = error;
    }
}

/// Ét forsøg: start sidecar, kør makro, log altid ud.
async fn attempt_item(app: &Arc<App>, cell_id: &str, settings: &Settings) -> Result<(), Fail> {
    let item = app.item(cell_id).ok_or_else(|| Fail::Stop("Ukendt celle".into()))?;
    let (join_macro, tokens) = {
        let data = app.data.lock().unwrap();
        let acc = data.accounts.iter().find(|a| a.id == item.account_id);
        let join = acc.and_then(|a| a.join_macro.clone()).unwrap_or_else(|| settings.join_macro.clone());
        let tokens = app.vault.lock().unwrap().tokens.get(&item.account_id).cloned().unwrap_or_default();
        (join, tokens)
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let sidecar = Sidecar::spawn(tx).await.map_err(|e| Fail::Stop(e.to_string()))?;
    let pump = tokio::spawn(pump_events(app.clone(), cell_id.to_string(), item.account_id.clone(), item.account_name.clone(), rx));

    let mut exec = Exec {
        app: app.clone(),
        sidecar: sidecar.clone(),
        cell_id: cell_id.to_string(),
        cell_name: item.cell_name.clone(),
        account_name: item.account_name.clone(),
        settings: settings.clone(),
        join_macro,
        connected: false,
        last_chat: vec![],
        clicks_needed: u32::from(settings.max_days),
    };

    let result = async {
        sidecar
            .request("init", json!({ "cache": Value::Object(tokens) }), Duration::from_secs(10))
            .await
            .map_err(|e| Fail::Stop(e.to_string()))?;
        exec.run_script(&settings.renew_macro, 0).await
    }
    .await;

    // Oprydning: altid logout, også ved fejl.
    app.update_item(cell_id, |i| i.step = "Logger ud".into());
    if exec.connected {
        app.log(cell_id, "info", "Logger ud");
    }
    sidecar.shutdown().await;
    drop(exec);
    drop(sidecar);
    let _ = tokio::time::timeout(Duration::from_secs(5), pump).await;
    result
}

/// Videresender events fra sidecaren til loggen, UI og det krypterede lager.
async fn pump_events(app: Arc<App>, cell_id: String, account_id: String, account_name: String, mut rx: mpsc::UnboundedReceiver<Event>) {
    while let Some(ev) = rx.recv().await {
        match ev {
            Event::Chat(t) => app.log(&cell_id, "chat", t),
            Event::Log(t) => app.log(&cell_id, "info", t),
            Event::Error(t) => app.log(&cell_id, "warn", t),
            Event::Status { state, detail } => {
                let text = match state.as_str() {
                    "connecting" => "Forbinder til serveren".to_string(),
                    "logged_in" => "Logget ind".to_string(),
                    "spawned" => "I spillet".to_string(),
                    "kicked" => format!("Smidt ud: {}", detail.unwrap_or_default()),
                    "ended" => format!("Forbindelse lukket: {}", detail.unwrap_or_default()),
                    _ => continue,
                };
                app.log(&cell_id, "info", text);
            }
            Event::MsaCode { code, url, expires_in } => {
                app.log(&cell_id, "warn", format!("Login for {account_name} er udløbet. Gå til {url} og skriv koden {code}"));
                let _ = app.handle.emit(
                    "msa-code",
                    json!({ "context": "run", "accountName": account_name, "code": code, "url": url, "expiresIn": expires_in }),
                );
            }
            Event::TokenCache { name, data } => {
                {
                    let mut vault = app.vault.lock().unwrap();
                    vault.tokens.entry(account_id.clone()).or_default().insert(name, data);
                }
                app.save_vault();
            }
            Event::Progress { done, total } => {
                app.update_item(&cell_id, |i| i.progress = Some((done, total)));
            }
            Event::Exited => {}
        }
    }
}

struct Exec {
    app: Arc<App>,
    sidecar: Arc<Sidecar>,
    cell_id: String,
    cell_name: String,
    account_name: String,
    settings: Settings,
    join_macro: String,
    connected: bool,
    last_chat: Vec<String>,
    clicks_needed: u32,
}

enum Slot {
    Before,
    After,
    Auto,
}

impl Exec {
    fn step(&self, text: &str) {
        self.app.update_item(&self.cell_id, |i| {
            i.step = text.to_string();
            i.progress = None;
        });
    }

    fn check_cancel(&self) -> Result<(), Fail> {
        if self.app.cancel.load(Ordering::SeqCst) {
            Err(Fail::Stop("Stoppet af brugeren".into()))
        } else {
            Ok(())
        }
    }

    async fn req(&self, cmd: &str, args: Value, timeout_ms: u64) -> Result<Value, Fail> {
        self.sidecar
            .request(cmd, args, Duration::from_millis(timeout_ms))
            .await
            .map_err(|e| {
                let msg = e.to_string();
                // Forbindelsesproblemer kan prøves igen; resten stopper.
                let retry = ["Forbindelsen", "stoppede", "Smidt ud", "ikke logget ind"];
                if retry.iter().any(|r| msg.contains(r)) {
                    Fail::Retry(msg)
                } else {
                    Fail::Stop(msg)
                }
            })
    }

    async fn run_script(&mut self, script: &str, depth: u8) -> Result<(), Fail> {
        for (n, raw) in macro_lang::lines(script) {
            self.check_cancel()?;
            let vars = Vars {
                cell: &self.cell_name,
                account: &self.account_name,
                max_days: self.settings.max_days,
                clicks_needed: self.clicks_needed,
            };
            let line = macro_lang::expand(&raw, &vars);
            let step = macro_lang::parse_line(&line).map_err(|e| Fail::Stop(format!("Makro, linje {n}: {e}")))?;
            match step {
                // Login sker ved waitjoin eller første kommando der kræver botten.
                Step::Start => {}
                Step::WaitJoin(ms) => self.connect(ms).await?,
                Step::JoinMacro => {
                    if depth > 0 {
                        return Err(Fail::Stop("joinmacro kan ikke bruges inde i join-makroen".into()));
                    }
                    self.step("Kører join-makro");
                    let join = self.join_macro.clone();
                    Box::pin(self.run_script(&join, depth + 1)).await?;
                }
                Step::Wait(ms) => tokio::time::sleep(Duration::from_millis(ms.min(600_000))).await,
                other => {
                    if !self.connected {
                        self.connect(self.settings.join_timeout_ms).await?;
                    }
                    self.exec(other).await?;
                }
            }
        }
        Ok(())
    }

    async fn connect(&mut self, join_timeout_ms: u64) -> Result<(), Fail> {
        if self.connected {
            return Ok(());
        }
        self.step("Logger ind");
        let s = &self.settings;
        let args = json!({ "host": s.host, "port": s.port, "version": s.version, "joinTimeoutMs": join_timeout_ms });
        // Ekstra tid hvis Microsoft beder om nyt login (kode i browseren).
        // Alle login-fejl må prøves igen (serveren kan være midlertidigt nede).
        let res = self
            .req("connect", args, join_timeout_ms + 16 * 60 * 1000)
            .await
            .map_err(|f| Fail::Retry(f.msg().to_string()))?;
        self.connected = true;
        let name = res.get("username").and_then(Value::as_str).unwrap_or(&self.account_name).to_string();
        self.app.log(&self.cell_id, "ok", format!("Logget ind som {name}"));
        {
            let mut data = self.app.data.lock().unwrap();
            if let Some(a) = data.accounts.iter_mut().find(|a| a.name.eq_ignore_ascii_case(&self.account_name)) {
                a.last_used = Some(Utc::now());
                a.last_error = None;
            }
        }
        Ok(())
    }

    async fn action(&self, args: Value) -> Result<(), Fail> {
        self.req("action", args, 35_000).await.map(|_| ())
    }

    async fn exec(&mut self, step: Step) -> Result<(), Fail> {
        match step {
            Step::Chat(text) => {
                self.req("chat", json!({ "text": text }), 10_000).await?;
                self.app.log(&self.cell_id, "info", format!("Sendte: {text}"));
            }
            Step::Hotbar(slot) => self.action(json!({ "action": "hotbar", "slot": slot })).await?,
            Step::Click { slot, button, mode } => {
                self.action(json!({ "action": "click", "slot": slot, "button": button, "mode": mode })).await?
            }
            Step::Move { action, ms } => self.action(json!({ "action": action, "ms": ms })).await?,
            Step::Action(a) => self.action(json!({ "action": a })).await?,
            Step::Sign { clicks, delay_ms, max_distance } => {
                let clicks = clicks.unwrap_or(u32::from(self.settings.max_days));
                self.sign(clicks, delay_ms, max_distance).await?;
            }
            Step::CheckChat { timeout_ms, command } => {
                let lower = command.to_lowercase();
                if let Some(rest) = lower.strip_prefix("/ce info") {
                    let target = command[command.len() - rest.len()..].trim();
                    let target = if target.is_empty() { self.cell_name.clone() } else { target.to_string() };
                    self.ce_info(&target, Slot::Auto).await?;
                } else {
                    let timeout = timeout_ms.unwrap_or(self.settings.chat_timeout_ms);
                    let res = self
                        .req("checkchat", json!({ "command": command, "timeoutMs": timeout }), timeout + 5_000)
                        .await?;
                    self.last_chat = lines_of(&res);
                }
            }
            Step::ValidateChat(v) => {
                let text = self.last_chat.join("\n");
                let ok = match &v {
                    Validation::Contains(s) => text.contains(s.as_str()),
                    Validation::Regex(r) => regex::Regex::new(r).map(|re| re.is_match(&text)).unwrap_or(false),
                };
                if !ok {
                    return Err(Fail::Stop(format!("Chat-validering fejlede ({v:?}). Svar: {}", text.replace('\n', " | "))));
                }
                self.app.log(&self.cell_id, "ok", "Chat-validering bestået");
            }
            Step::CeInfo(target) => {
                let target = target.unwrap_or_else(|| self.cell_name.clone());
                self.ce_info(&target, Slot::Auto).await?;
            }
            Step::Fill { rounds } => self.fill(rounds.unwrap_or(self.settings.fill_rounds).max(1)).await?,
            Step::Start | Step::WaitJoin(_) | Step::JoinMacro | Step::Wait(_) => {}
        }
        Ok(())
    }

    async fn sign(&mut self, clicks: u32, delay_ms: Option<u64>, max_distance: Option<u32>) -> Result<(), Fail> {
        if clicks == 0 {
            return Ok(());
        }
        self.step(&format!("Klikker på skilt ({clicks} klik)"));
        let s = &self.settings;
        let delay = delay_ms.unwrap_or(s.click_delay_ms);
        let args = json!({
            "clicks": clicks,
            "delayMs": delay,
            "jitterMs": s.click_jitter_ms,
            "maxDistance": max_distance.unwrap_or(s.max_distance),
        });
        let timeout = u64::from(clicks) * (delay + s.click_jitter_ms + 2_000) + 10_000;
        let res = self.req("sign", args, timeout).await?;
        let dist = res.get("distance").and_then(Value::as_f64).unwrap_or(0.0);
        let pos = res.get("position").cloned().unwrap_or(Value::Null);
        let text = res.get("text").and_then(Value::as_str).unwrap_or_default();
        self.app.log(
            &self.cell_id,
            "info",
            format!(
                "Skilt ved {} {} {} ({dist:.1} blokke væk){} – {clicks} klik sendt",
                pos["x"], pos["y"], pos["z"],
                if text.is_empty() { String::new() } else { format!(" «{text}»") }
            ),
        );
        if dist > 6.0 {
            self.app.log(&self.cell_id, "warn", "Skiltet er langt væk. Serveren kan afvise klik over ca. 8 blokke.");
        }
        self.app.update_item(&self.cell_id, |i| i.clicks += clicks);
        Ok(())
    }

    /// Sender `/ce info <mål>` og læser svaret. Prøver to gange ved timeout.
    async fn ce_info(&mut self, target: &str, slot: Slot) -> Result<CeInfo, Fail> {
        self.step(&format!("Tjekker /ce info {target}"));
        let is_cell = target.eq_ignore_ascii_case(&self.cell_name);
        let start = if is_cell { ceinfo::header_regex(target) } else { r"=\{".to_string() };
        let timeout = self.settings.chat_timeout_ms;
        let args = json!({
            "command": format!("/ce info {target}"),
            "timeoutMs": timeout,
            "settleMs": 800,
            "start": start,
            "until": ceinfo::END_REGEX,
        });

        let mut last_err = String::new();
        for attempt in 1..=2 {
            match self.req("checkchat", args.clone(), timeout + 5_000).await {
                Ok(res) => {
                    let lines = lines_of(&res);
                    self.last_chat = lines.clone();
                    let expected = if is_cell { Some(target) } else { None };
                    let info = ceinfo::parse(&lines, expected).map_err(Fail::Stop)?;
                    self.record_info(&info, slot);
                    return Ok(info);
                }
                Err(Fail::Stop(m)) if m.contains("Intet svar") && attempt == 1 => {
                    self.app.log(&self.cell_id, "warn", format!("{m} – prøver igen"));
                    last_err = m;
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(Fail::Stop(last_err))
    }

    fn record_info(&mut self, info: &CeInfo, slot: Slot) {
        let max = self.settings.max_days;
        self.clicks_needed = info.clicks_needed(max);
        let mut msg = format!("/ce info {}: {}", info.cell, info.time_text);
        if self.clicks_needed > 0 {
            msg.push_str(&format!(" – mangler {} dag(e)", self.clicks_needed));
        }
        self.app.log(&self.cell_id, "info", msg);
        if let Some(owner) = &info.owner {
            if !owner.eq_ignore_ascii_case(&self.account_name) {
                self.app.log(&self.cell_id, "warn", format!("Ejer ifølge serveren er {owner}, ikke {}", self.account_name));
            }
        }
        let info = info.clone();
        self.app.update_item(&self.cell_id, |i| match slot {
            // "Før" er altid den allerførste aflæsning i kørslen.
            Slot::Before if i.before.is_none() => i.before = Some(info),
            Slot::Before => {}
            Slot::After => i.after = Some(info),
            Slot::Auto if i.before.is_none() => i.before = Some(info),
            Slot::Auto => i.after = Some(info),
        });
    }

    /// Fyld op: læs dage, klik det der mangler, verificér, klik igen hvis nødvendigt.
    async fn fill(&mut self, rounds: u8) -> Result<(), Fail> {
        let max = self.settings.max_days;
        let mut current = self.ce_info(&self.cell_name.clone(), Slot::Before).await?;
        if current.is_full(max) {
            self.app.log(&self.cell_id, "ok", "Cellen er allerede fuld – ingen klik nødvendige");
            self.app.update_item(&self.cell_id, |i| i.after = Some(current.clone()));
            return Ok(());
        }
        for round in 1..=rounds {
            self.check_cancel()?;
            self.sign(current.clicks_needed(max), None, None).await?;
            let next = self.ce_info(&self.cell_name.clone(), Slot::After).await?;
            if next.is_full(max) {
                self.app.log(&self.cell_id, "ok", format!("Verificeret: {}", next.time_text));
                return Ok(());
            }
            if next.total_minutes() <= current.total_minutes() {
                return Err(Fail::Stop(
                    "Klikkene gav ingen ekstra dage. Skiltet er måske for langt væk, købet blev afvist, eller der mangler penge."
                        .into(),
                ));
            }
            if round < rounds {
                self.app.log(&self.cell_id, "warn", format!("Ikke fuld endnu ({}) – klikker igen", next.time_text));
            }
            current = next;
        }
        Ok(())
    }
}

fn lines_of(res: &Value) -> Vec<String> {
    res.get("lines")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Efter makroen: afgør resultat, gem på cellen og opdatér cell-manager.
async fn finish_item(app: &Arc<App>, cell_id: &str, settings: &Settings) {
    let Some(item) = app.item(cell_id) else { return };
    let info = item.after.clone().or(item.before.clone());
    let max = settings.max_days;

    let (state, error) = match &info {
        Some(i) if i.is_full(max) => (ItemState::Done, None),
        Some(i) => (ItemState::Warning, Some(format!("Kun {} efter køb (forventet {max} dage)", i.time_text))),
        None => (ItemState::Warning, Some("Ikke verificeret: makroen kørte ingen /ce info".into())),
    };

    let remote = {
        let mut data = app.data.lock().unwrap();
        let cell = data.cells.iter_mut().find(|c| c.id == cell_id);
        cell.map(|c| {
            if let Some(i) = &info {
                c.last_info = Some(i.clone());
                c.last_info_at = Some(Utc::now());
                if c.remote_id.is_none() {
                    c.days_left = Some(i.manager_days());
                }
            }
            if state == ItemState::Done && item.clicks > 0 {
                c.last_renewed = Some(Utc::now());
            }
            c.remote_id.is_some()
        })
        .unwrap_or(false)
    };
    app.save_data();
    set_account_error(app, cell_id, None);

    let sync = match (&info, state, remote) {
        (None, _, _) => SyncState::None,
        (Some(_), _, false) => SyncState::NotLinked,
        (Some(_), ItemState::Done, true) if settings.auto_sync => SyncState::Done,
        _ => SyncState::NeedsConfirm,
    };

    app.update_item(cell_id, |i| {
        i.state = state;
        i.step = match state {
            ItemState::Done => "Færdig".into(),
            _ => "Tjek advarsel".into(),
        };
        i.error = error.clone();
        i.sync = sync;
    });
    if let Some(e) = &error {
        app.log(cell_id, "error", e.clone());
    }

    if sync == SyncState::Done {
        let days = info.map(|i| i.manager_days()).unwrap_or(0);
        push_days(app, cell_id, days).await;
    }
}

/// Sender verificerede dage til cell-manager.
pub async fn push_days(app: &Arc<App>, cell_id: &str, days: u8) {
    let Some(item) = app.item(cell_id) else { return };
    let res = async { app.manager()?.set_days(&item.cell_name, days).await }.await;
    match res {
        Ok(()) => {
            app.log(cell_id, "ok", format!("cell-manager opdateret: {days} dage"));
            app.update_item(cell_id, |i| {
                i.sync = SyncState::Done;
                i.sync_error = None;
            });
        }
        Err(e) => {
            app.log(cell_id, "error", format!("cell-manager: {e}"));
            app.update_item(cell_id, |i| {
                i.sync = SyncState::Failed;
                i.sync_error = Some(e.to_string());
            });
        }
    }
}
