//! Krypteret lager til Microsoft-tokens og API-nøgle.
//!
//! - Lokalt: `vault.bin` krypteret med XChaCha20-Poly1305. Nøglen ligger i
//!   Windows Credential Manager / macOS Keychain (på Linux: `vault.key`, 0600).
//! - Eksport: kodeord → Argon2id → XChaCha20-Poly1305. Én fil, aldrig klartekst.

use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const VAULT_MAGIC: &[u8; 4] = b"CKV1";
const EXPORT_MAGIC: &[u8; 4] = b"CKX1";

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Vault {
    /// account-id → prismarine-auth-cache (`live`, `xbl`, `mca` …).
    pub tokens: HashMap<String, serde_json::Map<String, Value>>,
    pub api_key: Option<String>,
}

pub struct VaultStore {
    path: PathBuf,
    key: [u8; 32],
}

impl VaultStore {
    /// Åbner (eller opretter) lageret. Returnerer også en advarsel, hvis et
    /// gammelt lager ikke kunne læses (fx efter flytning til ny PC).
    pub fn open(dir: &Path) -> Result<(VaultStore, Vault, Option<String>)> {
        let key = load_or_create_key(dir)?;
        let store = VaultStore { path: dir.join("vault.bin"), key };
        if !store.path.exists() {
            return Ok((store, Vault::default(), None));
        }
        let bytes = fs::read(&store.path)?;
        match decrypt(&key, VAULT_MAGIC, &bytes).and_then(|p| Ok(serde_json::from_slice(&p)?)) {
            Ok(v) => Ok((store, v, None)),
            Err(_) => {
                let backup = dir.join(format!("vault.bin.ulæselig-{}", chrono::Utc::now().timestamp()));
                fs::rename(&store.path, &backup).ok();
                let warning = "Gemte logins kunne ikke læses på denne PC. Log ind igen eller importér en eksport-fil.".to_string();
                Ok((store, Vault::default(), Some(warning)))
            }
        }
    }

    pub fn save(&self, vault: &Vault) -> Result<()> {
        let plain = serde_json::to_vec(vault)?;
        let data = encrypt(&self.key, VAULT_MAGIC, &plain)?;
        write_atomic(&self.path, &data)
    }
}

fn encrypt(key: &[u8; 32], magic: &[u8; 4], plain: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plain)
        .map_err(|_| anyhow!("Kryptering fejlede"))?;
    let mut out = Vec::with_capacity(4 + 24 + ct.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn decrypt(key: &[u8; 32], magic: &[u8; 4], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 28 || &data[..4] != magic {
        bail!("Ukendt filformat");
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(&data[4..28]), &data[28..])
        .map_err(|_| anyhow!("Forkert kodeord eller beskadiget fil"))
}

fn password_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(key)
}

/// Fil-format: "CKX1" + salt(16) + nonce(24) + ciphertext.
pub fn export_encrypt(password: &str, plain: &[u8]) -> Result<Vec<u8>> {
    if password.chars().count() < 8 {
        bail!("Kodeordet skal være mindst 8 tegn");
    }
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = password_key(password, &salt)?;
    let body = encrypt(&key, EXPORT_MAGIC, plain)?;
    let mut out = Vec::with_capacity(16 + body.len());
    out.extend_from_slice(EXPORT_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&body[4..]);
    Ok(out)
}

pub fn is_export_file(data: &[u8]) -> bool {
    data.len() > 44 && &data[..4] == EXPORT_MAGIC
}

pub fn export_decrypt(password: &str, data: &[u8]) -> Result<Vec<u8>> {
    if !is_export_file(data) {
        bail!("Det er ikke en CellKeeper-eksportfil");
    }
    let key = password_key(password, &data[4..20])?;
    let mut body = EXPORT_MAGIC.to_vec();
    body.extend_from_slice(&data[20..]);
    decrypt(&key, EXPORT_MAGIC, &body)
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).with_context(|| format!("Kunne ikke skrive {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("Kunne ikke gemme {}", path.display()))?;
    Ok(())
}

fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    k
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

#[cfg(any(windows, target_os = "macos"))]
fn load_or_create_key(_dir: &Path) -> Result<[u8; 32]> {
    let entry = keyring::Entry::new("CellKeeper", "vault-key").context("Nøglering utilgængelig")?;
    match entry.get_password() {
        Ok(hex) => from_hex(&hex).ok_or_else(|| anyhow!("Ugyldig nøgle i nøgleringen")),
        Err(keyring::Error::NoEntry) => {
            let key = random_key();
            entry.set_password(&to_hex(&key)).context("Kunne ikke gemme nøgle i nøgleringen")?;
            Ok(key)
        }
        Err(e) => Err(anyhow!("Nøglering-fejl: {e}")),
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn load_or_create_key(dir: &Path) -> Result<[u8; 32]> {
    let path = dir.join("vault.key");
    if let Ok(hex) = fs::read_to_string(&path) {
        return from_hex(&hex).ok_or_else(|| anyhow!("Ugyldig vault.key"));
    }
    let key = random_key();
    fs::write(&path, to_hex(&key))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eksport_round_trip() {
        let data = export_encrypt("hemmeligt-kodeord", b"hej").unwrap();
        assert_eq!(export_decrypt("hemmeligt-kodeord", &data).unwrap(), b"hej");
        assert!(export_decrypt("forkert-kodeord", &data).is_err());
    }
}
