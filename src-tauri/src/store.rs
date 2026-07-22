//! Persisting the logged-in account across restarts.
//!
//! Written as plaintext JSON in the app-data dir. This matches how the game
//! itself (clientsettings.json) and VS Launcher (config.json) already store the
//! same session tokens, so it's no weaker than the status quo. A future
//! hardening pass could move it to the OS keychain.

use crate::models::Account;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

fn account_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("account.json"))
}

pub fn save_account(app: &AppHandle, account: &Account) -> Result<(), String> {
    let text = serde_json::to_string_pretty(account).map_err(|e| e.to_string())?;
    std::fs::write(account_path(app)?, text).map_err(|e| e.to_string())
}

pub fn load_account(app: &AppHandle) -> Option<Account> {
    let text = std::fs::read_to_string(account_path(app).ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn clear_account(app: &AppHandle) -> Result<(), String> {
    let p = account_path(app)?;
    if p.exists() {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    Ok(())
}
