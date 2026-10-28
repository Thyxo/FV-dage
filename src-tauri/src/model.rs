//! Data der gemmes i state.json (alt undtagen tokens og API-nøgle).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ceinfo::CeInfo;

pub const DEFAULT_JOIN_MACRO: &str = "# Åbn server-menuen og gå ind i fængslet\nuse\nwait 1000\nclick 20\nwait 15000";

pub const DEFAULT_RENEW_MACRO: &str = "# Log ind på serveren\nstart\nwaitjoin 30000\njoinmacro\n# Køb dage til cellen er fuld, og tjek med /ce info\nfyldop";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    /// Minecraft-navn (fra Microsoft-login).
    pub name: String,
    /// Valgfrit kaldenavn i appen (ændrer ikke Minecraft-navnet).
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Egen join-makro. `None` = brug den globale.
    #[serde(default)]
    pub join_macro: Option<String>,
    #[serde(default)]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    /// Lokalt id.
    pub id: String,
    pub cell_name: String,
    /// Minecraft-navnet på ejeren (matches mod accounts uden forskel på store/små bogstaver).
    pub account_name: String,
    /// cell-managers `_id`. `None` = kun lokal (sendes aldrig til cell-manager).
    #[serde(default)]
    pub remote_id: Option<String>,
    #[serde(default)]
    pub rank: String,
    #[serde(default)]
    pub block: String,
    /// Dage ifølge cell-manager (eller seneste verificerede værdi for lokale celler).
    #[serde(default)]
    pub days_left: Option<u8>,
    #[serde(default)]
    pub removed_remote: bool,
    #[serde(default)]
    pub last_renewed: Option<DateTime<Utc>>,
    /// Seneste /ce info-aflæsning.
    #[serde(default)]
    pub last_info: Option<CeInfo>,
    #[serde(default)]
    pub last_info_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub version: String,
    /// Fuld celle = så mange dage (cell-manager tillader 0-9).
    pub max_days: u8,
    pub click_delay_ms: u64,
    pub click_jitter_ms: u64,
    pub max_distance: u32,
    /// Hvor mange klik-runder "fyldop" må bruge, før den giver op.
    pub fill_rounds: u8,
    pub join_timeout_ms: u64,
    pub chat_timeout_ms: u64,
    pub max_parallel: u8,
    pub login_gap_ms: u64,
    pub login_jitter_ms: u64,
    /// Automatiske genforsøg pr. celle ved forbindelsesfejl.
    pub retries: u8,
    pub join_macro: String,
    pub renew_macro: String,
    pub manager_url: String,
    /// Opdatér cell-manager automatisk efter verificeret køb.
    pub auto_sync: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            host: "mc.freakyville.dk".into(),
            port: 25565,
            version: "1.8.9".into(),
            max_days: 9,
            click_delay_ms: 300,
            click_jitter_ms: 80,
            max_distance: 8,
            fill_rounds: 3,
            join_timeout_ms: 30_000,
            chat_timeout_ms: 5_000,
            max_parallel: 2,
            login_gap_ms: 5_000,
            login_jitter_ms: 2_000,
            retries: 1,
            join_macro: DEFAULT_JOIN_MACRO.into(),
            renew_macro: DEFAULT_RENEW_MACRO.into(),
            manager_url: "https://backend-production-a068.up.railway.app".into(),
            auto_sync: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Data {
    pub accounts: Vec<Account>,
    pub cells: Vec<Cell>,
    pub settings: Settings,
}

impl Data {
    pub fn account_for_cell(&self, cell: &Cell) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(cell.account_name.trim()))
    }
}

fn yes() -> bool {
    true
}

pub fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}
