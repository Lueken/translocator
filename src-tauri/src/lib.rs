//! Translocator — Tauri command surface.
//!
//! Crown jewel: log in once and launch any installation repeatedly WITHOUT
//! being bounced to re-login (where VS Launcher and StoryForge both fail) via
//! validate-before-stamp (`play` step 1) and read-back-after-exit (`play`
//! step 4). Also: persisted account (survives restart) and ModDB mod install.

mod auth;
mod launch;
mod models;
mod mods;
mod session;
mod store;

use models::Account;
use serde::Serialize;
use std::path::PathBuf;
use tauri::AppHandle;

/// Result of a login attempt, tagged for the frontend.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum LoginOutcome {
    Success { account: Account },
    NeedsTotp { prelogintoken: String, reason: Option<String> },
    Failed { reason: String },
}

/// POST /v2/gamelogin, mapped into a frontend-friendly outcome. On success the
/// account is persisted so it survives an app restart.
#[tauri::command]
async fn login(
    app: AppHandle,
    email: String,
    password: String,
    totp: Option<String>,
    prelogintoken: Option<String>,
) -> Result<LoginOutcome, String> {
    let r = auth::game_login(&email, &password, totp.as_deref(), prelogintoken.as_deref()).await?;

    if r.valid == 1 {
        let account = Account {
            uid: r.uid.unwrap_or_default(),
            playername: r.playername.unwrap_or_default(),
            email,
            sessionkey: r.sessionkey.unwrap_or_default(),
            sessionsignature: r.sessionsignature.unwrap_or_default(),
            mptoken: r.mptoken,
            entitlements: r.entitlements,
        };
        store::save_account(&app, &account)?;
        Ok(LoginOutcome::Success { account })
    } else if let Some(pt) = r.prelogintoken {
        Ok(LoginOutcome::NeedsTotp {
            prelogintoken: pt,
            reason: r.reason,
        })
    } else {
        Ok(LoginOutcome::Failed {
            reason: r.reason.unwrap_or_else(|| "Login failed".into()),
        })
    }
}

/// The persisted account, if any (called on app startup to restore the session).
#[tauri::command]
fn get_account(app: AppHandle) -> Option<Account> {
    store::load_account(&app)
}

/// Forget the persisted account.
#[tauri::command]
fn logout(app: AppHandle) -> Result<(), String> {
    store::clear_account(&app)
}

#[derive(Serialize)]
struct InstallInfo {
    name: String,
    path: String,
    /// Whether this install already has a well-formed session written in.
    has_session: bool,
}

/// List each subdirectory of `installations_dir` as an installation (its
/// `--dataPath`). Mirrors how VS Launcher lays out its installations folder.
#[tauri::command]
fn list_installs(installations_dir: String) -> Result<Vec<InstallInfo>, String> {
    let base = PathBuf::from(&installations_dir);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&base).map_err(|e| format!("read_dir failed: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let path = entry.path();
            out.push(InstallInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                has_session: session::read_back_session(&path).is_some(),
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Result of a play attempt.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum PlayResult {
    /// The stored session is dead server-side; the UI must re-login.
    NeedsRelogin { reason: String },
    /// The game ran. If `rotated`, the game issued a new session on exit and
    /// `account` is the refreshed copy the UI should persist.
    Played {
        exit_code: i32,
        rotated: bool,
        account: Account,
    },
}

/// The crown jewel: validate -> stamp -> launch -> read-back.
#[tauri::command]
async fn play(
    app: AppHandle,
    game_exe: String,
    install_dir: String,
    account: Account,
    start_params: Option<String>,
) -> Result<PlayResult, String> {
    // 1. Validate the stored session with the account server BEFORE trusting it.
    let v = auth::client_validate(&account.uid, &account.sessionkey).await?;
    if v.valid != 1 {
        return Ok(PlayResult::NeedsRelogin {
            reason: v.reason.unwrap_or_else(|| "nosession".into()),
        });
    }

    // 2. Stamp the (validated) session into the install's clientsettings.json.
    let dir = PathBuf::from(&install_dir);
    session::stamp_session(&dir, &account)?;

    // 3. Launch and wait for the game to close.
    let exit_code = launch::launch_and_wait(&game_exe, &dir, start_params.as_deref()).await?;

    // 4. Read back the session the game left behind. If it rotated, capture and
    //    persist it so we never re-stamp a stale key on the next launch.
    let mut refreshed = account.clone();
    let mut rotated = false;
    if let Some((uid, key, sig, name)) = session::read_back_session(&dir) {
        if key != account.sessionkey || sig != account.sessionsignature {
            rotated = true;
            refreshed.uid = uid;
            refreshed.sessionkey = key;
            refreshed.sessionsignature = sig;
            if !name.is_empty() {
                refreshed.playername = name;
            }
            store::save_account(&app, &refreshed)?;
        }
    }

    Ok(PlayResult::Played {
        exit_code,
        rotated,
        account: refreshed,
    })
}

/// Text-search ModDB (most-downloaded first).
#[tauri::command]
async fn search_mods(text: String) -> Result<Vec<mods::ModSummary>, String> {
    mods::search(&text).await
}

/// Download a mod's latest release into the install's Mods folder.
#[tauri::command]
async fn install_mod(install_dir: String, modidstr: String) -> Result<String, String> {
    mods::install_latest(&PathBuf::from(install_dir), &modidstr).await
}

/// Author donation link(s) for a mod — the structured `donateurl` field if the
/// API exposes it, otherwise parsed from the description. Reference-only.
#[tauri::command]
async fn mod_donations(modidstr: String) -> Result<Vec<String>, String> {
    mods::get_donations(&modidstr).await
}

/// Zip filenames currently in an install's Mods folder.
#[tauri::command]
fn list_mod_files(install_dir: String) -> Result<Vec<String>, String> {
    Ok(mods::list_mod_files(&PathBuf::from(install_dir)))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            login,
            get_account,
            logout,
            list_installs,
            play,
            search_mods,
            install_mod,
            mod_donations,
            list_mod_files
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
