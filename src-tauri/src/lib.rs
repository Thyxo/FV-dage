mod app;
mod ceinfo;
mod cellmanager;
mod macro_lang;
mod model;
mod run;
mod sidecar;
mod vault;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager as _, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{mpsc, oneshot};

use app::{App, View};
use model::{new_id, Account, Cell, Settings};
use run::LogLine;
use sidecar::{Event, Sidecar};

type Res<T> = Result<T, String>;
type AppState<'a> = State<'a, Arc<App>>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn get_view(app: AppState) -> View {
    app.view()
}

#[tauri::command]
fn dismiss_notice(app: AppState) -> View {
    *app.notice.lock().unwrap() = None;
    app.view()
}

#[tauri::command]
fn save_settings(app: AppState, settings: Settings, api_key: Option<String>) -> Res<View> {
    for (name, script) in [("Join-makro", &settings.join_macro), ("Fornyelses-makro", &settings.renew_macro)] {
        if let Some(e) = macro_lang::validate(script).first() {
            return Err(format!("{name}: {e}"));
        }
    }
    if !(1..=9).contains(&settings.max_days) {
        return Err("Fuld celle skal være 1-9 dage".into());
    }
    app.data.lock().unwrap().settings = settings;
    app.save_data();
    if let Some(key) = api_key {
        app.vault.lock().unwrap().api_key = Some(key.trim().to_string()).filter(|k| !k.is_empty());
        app.save_vault();
    }
    Ok(app.view())
}

#[tauri::command]
fn default_settings() -> Settings {
    Settings::default()
}

#[tauri::command]
fn validate_macro(script: String) -> Vec<String> {
    macro_lang::validate(&script)
}

#[tauri::command]
async fn test_manager(url: String, api_key: Option<String>, app: AppState<'_>) -> Res<String> {
    let key = api_key.or_else(|| app.vault.lock().unwrap().api_key.clone());
    cellmanager::Manager::new(&url, key).map_err(err)?.test().await.map_err(err)
}

#[tauri::command]
async fn sync_cells(app: AppState<'_>) -> Res<View> {
    app.sync_cells().await.map_err(err)?;
    Ok(app.view())
}

// ── Accounts ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Profile {
    name: String,
    uuid: Option<String>,
}

/// Microsoft-login med kode (device code). Med `account_id` fornyes login
/// for en eksisterende account, ellers oprettes en ny.
#[tauri::command]
async fn login_account(app: AppState<'_>, account_id: Option<String>) -> Res<View> {
    if app.run_active.load(Ordering::SeqCst) {
        return Err("Vent til kørslen er færdig".into());
    }
    let app = app.inner().clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sc = Sidecar::spawn(tx).await.map_err(err)?;
    *app.login.lock().await = Some(sc.clone());

    let collected: Arc<Mutex<Map<String, Value>>> = Arc::default();
    let pump_tokens = collected.clone();
    let pump_app = app.clone();
    let pump = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::TokenCache { name, data } => {
                    pump_tokens.lock().unwrap().insert(name, data);
                }
                Event::MsaCode { code, url, expires_in } => {
                    let _ = pump_app.handle.emit(
                        "msa-code",
                        json!({ "context": "login", "code": code, "url": url, "expiresIn": expires_in }),
                    );
                }
                _ => {}
            }
        }
    });

    let result = async {
        sc.request("init", json!({ "cache": {} }), Duration::from_secs(10)).await?;
        sc.request("auth", json!({}), Duration::from_secs(16 * 60)).await
    }
    .await;
    sc.kill().await;
    drop(sc);
    // cancel_login har allerede taget sidecaren, hvis brugeren annullerede.
    let cancelled = app.login.lock().await.take().is_none();
    let _ = tokio::time::timeout(Duration::from_secs(5), pump).await;
    let _ = app.handle.emit("msa-code-done", json!({}));
    if cancelled {
        return Err("Login annulleret".into());
    }

    let profile: Profile = serde_json::from_value(result.map_err(|e| format!("Login fejlede: {e}"))?).map_err(err)?;
    let tokens = std::mem::take(&mut *collected.lock().unwrap());

    let id = {
        let mut data = app.data.lock().unwrap();
        let existing = data.accounts.iter_mut().find(|a| {
            Some(&a.id) == account_id.as_ref()
                || (a.uuid.is_some() && a.uuid == profile.uuid)
                || a.name.eq_ignore_ascii_case(&profile.name)
        });
        match existing {
            Some(a) => {
                a.name = profile.name.clone();
                a.uuid = profile.uuid.clone();
                a.last_error = None;
                a.id.clone()
            }
            None => {
                let a = Account {
                    id: new_id(),
                    name: profile.name.clone(),
                    label: None,
                    uuid: profile.uuid.clone(),
                    enabled: true,
                    join_macro: None,
                    last_used: None,
                    last_error: None,
                };
                let id = a.id.clone();
                data.accounts.push(a);
                id
            }
        }
    };
    app.vault.lock().unwrap().tokens.insert(id, tokens);
    app.save_vault();
    app.save_data();
    Ok(app.view())
}

#[tauri::command]
async fn cancel_login(app: AppState<'_>) -> Res<()> {
    let sc = app.login.lock().await.take();
    if let Some(sc) = sc {
        sc.kill().await;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountPatch {
    label: Option<String>,
    enabled: Option<bool>,
    /// `Some("")` = brug den globale join-makro.
    join_macro: Option<String>,
}

#[tauri::command]
fn update_account(app: AppState, id: String, patch: AccountPatch) -> Res<View> {
    if let Some(script) = &patch.join_macro {
        if let Some(e) = macro_lang::validate(script).first() {
            return Err(format!("Join-makro: {e}"));
        }
    }
    {
        let mut data = app.data.lock().unwrap();
        let a = data.accounts.iter_mut().find(|a| a.id == id).ok_or("Ukendt account")?;
        if let Some(l) = patch.label {
            a.label = Some(l.trim().to_string()).filter(|l| !l.is_empty());
        }
        if let Some(e) = patch.enabled {
            a.enabled = e;
        }
        if let Some(m) = patch.join_macro {
            a.join_macro = Some(m).filter(|m| !m.trim().is_empty());
        }
    }
    app.save_data();
    Ok(app.view())
}

#[tauri::command]
fn remove_account(app: AppState, id: String) -> View {
    app.data.lock().unwrap().accounts.retain(|a| a.id != id);
    app.vault.lock().unwrap().tokens.remove(&id);
    app.save_data();
    app.save_vault();
    app.view()
}

// ── Celler ───────────────────────────────────────────────────────────────

#[tauri::command]
fn add_local_cell(app: AppState, cell_name: String, account_name: String) -> Res<View> {
    let cell_name = cell_name.trim().to_string();
    let account_name = account_name.trim().to_string();
    if cell_name.is_empty() || account_name.is_empty() {
        return Err("Udfyld cellenavn og account".into());
    }
    {
        let mut data = app.data.lock().unwrap();
        if data.cells.iter().any(|c| c.cell_name.eq_ignore_ascii_case(&cell_name)) {
            return Err(format!("Cellen {cell_name} findes allerede"));
        }
        data.cells.push(Cell {
            id: new_id(),
            cell_name,
            account_name,
            remote_id: None,
            rank: String::new(),
            block: String::new(),
            days_left: None,
            removed_remote: false,
            last_renewed: None,
            last_info: None,
            last_info_at: None,
        });
    }
    app.save_data();
    Ok(app.view())
}

#[tauri::command]
fn remove_cell(app: AppState, id: String) -> Res<View> {
    {
        let mut data = app.data.lock().unwrap();
        let cell = data.cells.iter().find(|c| c.id == id).ok_or("Ukendt celle")?;
        if cell.remote_id.is_some() && !cell.removed_remote {
            return Err("Cellen hører til cell-manager. Slet den dér i stedet.".into());
        }
        data.cells.retain(|c| c.id != id);
    }
    app.save_data();
    Ok(app.view())
}

// ── Kørsel ───────────────────────────────────────────────────────────────

#[tauri::command]
fn start_run(app: AppState, cell_ids: Vec<String>) -> Res<View> {
    if cell_ids.is_empty() {
        return Err("Vælg mindst én celle".into());
    }
    if app.run_active.swap(true, Ordering::SeqCst) {
        return Err("En kørsel er allerede i gang".into());
    }
    let items = {
        let data = app.data.lock().unwrap();
        let vault = app.vault.lock().unwrap();
        let mut items = Vec::new();
        for id in &cell_ids {
            let Some(cell) = data.cells.iter().find(|c| &c.id == id) else { continue };
            let Some(acc) = data.account_for_cell(cell) else { continue };
            if !acc.enabled || !vault.tokens.get(&acc.id).is_some_and(|t| !t.is_empty()) {
                continue;
            }
            items.push(run::new_item(&cell.id, &cell.cell_name, &acc.id, &acc.name));
        }
        items
    };
    if items.is_empty() {
        app.run_active.store(false, Ordering::SeqCst);
        return Err("Ingen af de valgte celler har en account med login".into());
    }
    let ids: Vec<String> = items.iter().map(|i| i.cell_id.clone()).collect();
    *app.run.lock().unwrap() = Some(run::new_run(items));
    let a = app.inner().clone();
    tauri::async_runtime::spawn(async move { run::execute(a, ids).await });
    Ok(app.view())
}

/// Genoptag/prøv igen for udvalgte celler i den seneste kørsel.
#[tauri::command]
fn retry_items(app: AppState, cell_ids: Vec<String>) -> Res<View> {
    if app.run_active.swap(true, Ordering::SeqCst) {
        return Err("En kørsel er allerede i gang".into());
    }
    let ids: Vec<String> = {
        let guard = app.run.lock().unwrap();
        let run = guard.as_ref();
        let known = |id: &String| run.is_some_and(|r| r.items.iter().any(|i| &i.cell_id == id));
        cell_ids.into_iter().filter(known).collect()
    };
    if ids.is_empty() {
        app.run_active.store(false, Ordering::SeqCst);
        return Err("Intet at genoptage".into());
    }
    let a = app.inner().clone();
    tauri::async_runtime::spawn(async move { run::execute(a, ids).await });
    Ok(app.view())
}

#[tauri::command]
fn cancel_run(app: AppState) {
    app.cancel.store(true, Ordering::SeqCst);
}

/// Brugeren bekræfter at sende en ikke-fuld (men verificeret) værdi til cell-manager.
#[tauri::command]
async fn confirm_sync(app: AppState<'_>, cell_id: String) -> Res<View> {
    let item = app.item(&cell_id).ok_or("Ukendt celle")?;
    let info = item.after.or(item.before).ok_or("Ingen verificeret værdi")?;
    let a = app.inner().clone();
    run::push_days(&a, &cell_id, info.manager_days()).await;
    let _ = app.sync_cells().await;
    Ok(app.view())
}

#[tauri::command]
fn item_log(app: AppState, cell_id: String) -> Vec<LogLine> {
    app.run
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|r| r.items.iter().find(|i| i.cell_id == cell_id).map(|i| i.log.clone()))
        .unwrap_or_default()
}

// ── Eksport / import ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportFile {
    format: String,
    version: u32,
    exported_at: String,
    accounts: Vec<Account>,
    tokens: std::collections::HashMap<String, Map<String, Value>>,
    settings: Settings,
    api_key: Option<String>,
}

async fn pick_save(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .add_filter("CellKeeper-eksport", &["cellkeeper"])
        .set_file_name("cellkeeper-accounts.cellkeeper")
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    rx.await.ok().flatten().and_then(|p| p.into_path().ok())
}

async fn pick_open(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .add_filter("CellKeeper-eksport eller accounts.json", &["cellkeeper", "json"])
        .pick_file(move |p| {
            let _ = tx.send(p);
        });
    rx.await.ok().flatten().and_then(|p| p.into_path().ok())
}

#[tauri::command]
async fn export_accounts(app: AppState<'_>, password: String) -> Res<String> {
    let file = {
        let data = app.data.lock().unwrap();
        let vault = app.vault.lock().unwrap();
        ExportFile {
            format: "cellkeeper-export".into(),
            version: 1,
            exported_at: chrono::Utc::now().to_rfc3339(),
            accounts: data.accounts.clone(),
            tokens: vault.tokens.clone(),
            settings: data.settings.clone(),
            api_key: vault.api_key.clone(),
        }
    };
    let plain = serde_json::to_vec(&file).map_err(err)?;
    let bytes = vault::export_encrypt(&password, &plain).map_err(err)?;
    let Some(path) = pick_save(&app.handle).await else {
        return Ok("Eksport annulleret".into());
    };
    std::fs::write(&path, bytes).map_err(err)?;
    Ok(format!("{} accounts eksporteret til {}", file.accounts.len(), path.display()))
}

#[tauri::command]
async fn import_accounts(app: AppState<'_>, password: String) -> Res<String> {
    let Some(path) = pick_open(&app.handle).await else {
        return Ok("Import annulleret".into());
    };
    let bytes = std::fs::read(&path).map_err(err)?;

    if vault::is_export_file(&bytes) {
        let plain = vault::export_decrypt(&password, &bytes).map_err(err)?;
        let file: ExportFile = serde_json::from_slice(&plain).map_err(err)?;
        let count = file.accounts.len();
        {
            let mut data = app.data.lock().unwrap();
            let mut vault = app.vault.lock().unwrap();
            for acc in file.accounts {
                let tokens = file.tokens.get(&acc.id).cloned().unwrap_or_default();
                let id = match data.accounts.iter_mut().find(|a| {
                    (a.uuid.is_some() && a.uuid == acc.uuid) || a.name.eq_ignore_ascii_case(&acc.name)
                }) {
                    Some(existing) => {
                        existing.join_macro = acc.join_macro.clone().or(existing.join_macro.take());
                        existing.id.clone()
                    }
                    None => {
                        let id = acc.id.clone();
                        data.accounts.push(acc);
                        id
                    }
                };
                if !tokens.is_empty() {
                    vault.tokens.insert(id, tokens);
                }
            }
            if vault.api_key.is_none() {
                vault.api_key = file.api_key;
            }
            if data.accounts.len() == count {
                // Ny PC uden egne indstillinger: tag dem fra eksporten.
                data.settings = file.settings;
            }
        }
        app.save_data();
        app.save_vault();
        app.emit_view();
        return Ok(format!("{count} accounts importeret med login"));
    }

    // headless-clients accounts.json (objekt eller liste, evt. med BOM).
    let text = String::from_utf8_lossy(&bytes);
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "Filen er hverken en CellKeeper-eksport eller en accounts.json".to_string())?;
    let entries = match value {
        Value::Array(a) => a,
        v => vec![v],
    };
    let mut added = 0;
    {
        let mut data = app.data.lock().unwrap();
        for e in entries {
            let cfg = e.get("config").unwrap_or(&e);
            let s = |k: &str| cfg.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
            let Some(name) = s("avatar").or(s("name")) else { continue };
            if data.accounts.iter().any(|a| a.name.eq_ignore_ascii_case(name)) {
                continue;
            }
            data.accounts.push(Account {
                id: new_id(),
                name: name.to_string(),
                label: None,
                uuid: None,
                enabled: true,
                join_macro: s("joinMacro").map(String::from),
                last_used: None,
                last_error: None,
            });
            added += 1;
        }
    }
    app.save_data();
    app.emit_view();
    Ok(format!("{added} accounts importeret fra accounts.json. De skal logge ind (knappen \"Log ind\")."))
}

#[tauri::command]
fn open_url(app: AppState, url: String) -> Res<()> {
    if !url.starts_with("https://") {
        return Err("Kun https-links".into());
    }
    app.handle.opener().open_url(url, None::<&str>).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let state = Arc::new(App::load(app.handle().clone(), dir)?);
            app.manage(state.clone());
            // Hent celler ved start og derefter hvert andet minut.
            tauri::async_runtime::spawn(async move {
                loop {
                    let _ = state.sync_cells().await;
                    tokio::time::sleep(Duration::from_secs(120)).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_view,
            dismiss_notice,
            save_settings,
            default_settings,
            validate_macro,
            test_manager,
            sync_cells,
            login_account,
            cancel_login,
            update_account,
            remove_account,
            add_local_cell,
            remove_cell,
            start_run,
            retry_items,
            cancel_run,
            confirm_sync,
            item_log,
            export_accounts,
            import_accounts,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("CellKeeper kunne ikke starte");
}
