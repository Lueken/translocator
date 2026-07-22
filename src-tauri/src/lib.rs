//! Translocator - Tauri command surface.
//!
//! Crown jewel: log in once and launch any installation repeatedly WITHOUT
//! being bounced to re-login (where VS Launcher and StoryForge both fail) via
//! validate-before-stamp (`play` step 1) and read-back-after-exit (`play`
//! step 4). Also: persisted account (survives restart) and ModDB mod install.

mod auth;
mod backup;
mod deps;
mod installations;
mod launch;
mod migrate;
mod models;
mod mods;
mod session;
mod store;
mod updates;
mod versions;
mod worlds;

use models::Account;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

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

/// Open a URL in the user's default browser (e.g. a mod's ModDB page).
#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Forget the persisted account.
#[tauri::command]
fn logout(app: AppHandle) -> Result<(), String> {
    store::clear_account(&app)
}

/// Every installation under `installations_dir`, with its metadata (adopting any
/// folder we don't yet track). `default_version` is used only when adopting a
/// folder that has no metadata yet.
#[tauri::command]
fn list_installations(
    installations_dir: String,
    default_version: String,
) -> Result<Vec<installations::InstallationCard>, String> {
    installations::list(&PathBuf::from(installations_dir), &default_version)
}

/// Persist edited installation metadata (rename, params, env, backup prefs,
/// favorite, icon).
#[tauri::command]
fn save_installation(path: String, meta: installations::InstallationMeta) -> Result<(), String> {
    installations::write_meta(&PathBuf::from(path), &meta)
}

/// Delete an installation folder. The UI confirms first.
#[tauri::command]
fn delete_installation(path: String) -> Result<(), String> {
    installations::delete(&PathBuf::from(path))
}

/// Reveal an installation's folder in the OS file browser.
#[tauri::command]
fn open_install_folder(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}

/// Create a new installation folder pinned to `version` (the version should be
/// ensured/downloaded first). If `seed_from` is set, copy game settings
/// (keybinds/graphics, minus auth) from it: a real path, or "__base__" for the
/// base game's VintagestoryData. Returns the new folder path.
#[tauri::command]
fn create_installation(
    app: AppHandle,
    installations_dir: String,
    name: String,
    version: String,
    seed_from: Option<String>,
) -> Result<String, String> {
    use tauri::Manager;
    let path = installations::create(&PathBuf::from(&installations_dir), &name, &version)?;
    if let Some(src) = seed_from {
        let source = if src == "__base__" {
            app.path()
                .data_dir()
                .map_err(|e| e.to_string())?
                .join("VintagestoryData")
        } else {
            PathBuf::from(src)
        };
        let _ = installations::seed_clientsettings(&source, &PathBuf::from(&path));
    }
    Ok(path)
}

// ---- game versions (shared, deduplicated binary cache) ----

/// Installable Windows versions from the official manifest, newest first, each
/// flagged if already cached.
#[tauri::command]
async fn list_available_versions(app: AppHandle) -> Result<Vec<versions::AvailableVersion>, String> {
    versions::fetch_available(&app).await
}

/// Cached version strings (downloaded + installed).
#[tauri::command]
fn list_cached_versions(app: AppHandle) -> Vec<String> {
    versions::list_cached(&app)
}

/// Ensure a version is in the cache (download + silent install if needed).
/// Emits "version-progress". Returns the cached exe path.
#[tauri::command]
async fn ensure_version(app: AppHandle, version: String, url: String) -> Result<String, String> {
    versions::ensure_version(&app, &version, &url).await
}

/// Remove a cached version's binaries.
#[tauri::command]
fn remove_version(app: AppHandle, version: String) -> Result<(), String> {
    versions::remove_cached(&app, &version)
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

/// The crown jewel: validate -> stamp -> launch -> read-back. Launch params,
/// env vars, and auto-backup come from the installation's own metadata.
#[tauri::command]
async fn play(
    app: AppHandle,
    game_exe: String,
    install_dir: String,
    account: Account,
) -> Result<PlayResult, String> {
    // 1. Validate the stored session with the account server BEFORE trusting it.
    let v = auth::client_validate(&account.uid, &account.sessionkey).await?;
    if v.valid != 1 {
        return Ok(PlayResult::NeedsRelogin {
            reason: v.reason.unwrap_or_else(|| "nosession".into()),
        });
    }

    let dir = PathBuf::from(&install_dir);
    let meta = installations::read_meta(&dir).unwrap_or_default();

    // Resolve the game binary: prefer our cached copy for the install's pinned
    // version; fall back to the caller-provided exe (e.g. an existing VS
    // Launcher binary) so already-set-up installs keep working.
    let exe = if !meta.version.is_empty() {
        versions::exe_path(&app, &meta.version)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(game_exe)
    } else {
        game_exe
    };
    if !std::path::Path::new(&exe).exists() {
        return Err(format!(
            "Game {} isn't installed. Open this installation's settings to download it.",
            if meta.version.is_empty() { "binary".into() } else { meta.version.clone() }
        ));
    }

    // 2. Optional whole-install backup before playing (off the UI thread), then
    //    stamp the validated session.
    if meta.auto_backup {
        let (d, comp, keep) = (dir.clone(), meta.compression, meta.backups_limit as usize);
        let _ = tokio::task::spawn_blocking(move || backup::backup_install(&d, comp, keep)).await;
    }
    session::stamp_session(&dir, &account)?;

    // 3. Launch with the install's params/env and wait; time the session.
    let started = std::time::Instant::now();
    let exit_code = launch::launch_and_wait(
        &exe,
        &dir,
        Some(&meta.start_params),
        Some(&meta.env_vars),
    )
    .await?;
    installations::record_play(&dir, started.elapsed().as_secs());

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
async fn install_mod(app: AppHandle, install_dir: String, modidstr: String) -> Result<String, String> {
    mods::install_latest(&app, &PathBuf::from(install_dir), &modidstr).await
}

/// Author donation link(s) for a mod - the structured `donateurl` field if the
/// API exposes it, otherwise parsed from the description. Reference-only.
#[tauri::command]
async fn mod_donations(modidstr: String) -> Result<Vec<String>, String> {
    mods::get_donations(&modidstr).await
}

/// Installed mods with newer ModDB releases, each release carrying its
/// compatibility (vs `game_version`) and changelog. Emits "check-progress".
#[tauri::command]
async fn check_updates(
    app: AppHandle,
    install_dir: String,
    game_version: String,
) -> Result<Vec<updates::ModUpdate>, String> {
    updates::check_updates(&app, &PathBuf::from(install_dir), &game_version).await
}

/// Install a specific release version, replacing the currently installed zip.
#[tauri::command]
async fn install_release(
    app: AppHandle,
    install_dir: String,
    modidstr: String,
    modversion: String,
    old_filename: Option<String>,
) -> Result<String, String> {
    mods::install_release(
        &app,
        &PathBuf::from(install_dir),
        &modidstr,
        &modversion,
        old_filename.as_deref(),
    )
    .await
}

/// Required dependencies of an installed mod zip that are missing from the
/// install (parsed from modinfo.json; skips the base game).
#[tauri::command]
fn check_deps(install_dir: String, filename: String) -> Result<Vec<deps::MissingDep>, String> {
    let base = PathBuf::from(&install_dir);
    let zip = base.join("Mods").join(&filename);
    Ok(deps::check_missing_deps(&base, &zip))
}

/// Zip filenames currently in an install's Mods folder.
#[tauri::command]
fn list_mod_files(install_dir: String) -> Result<Vec<String>, String> {
    Ok(mods::list_mod_files(&PathBuf::from(install_dir)))
}

/// Snapshot the install's Mods folder into a fresh timestamped backup; returns
/// the backup id. Called before applying mod updates (fast, mods-only). Runs off
/// the UI thread.
#[tauri::command]
async fn backup_mods(install_dir: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || backup::backup_mods(&PathBuf::from(install_dir)))
        .await
        .map_err(|e| format!("backup task failed: {e}"))?
}

/// Snapshot the WHOLE installation (worlds, config, mods) into a compressed
/// backup at the given level, pruned to `keep`. Heavy (gigabytes), so it runs on
/// a blocking thread to keep the UI responsive. Returns the backup id.
#[tauri::command]
async fn backup_install(install_dir: String, compression: u8, keep: u8) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        backup::backup_install(&PathBuf::from(install_dir), compression, keep as usize)
    })
    .await
    .map_err(|e| format!("backup task failed: {e}"))?
}

/// All backups for an install, newest-first.
#[tauri::command]
fn list_backups(install_dir: String) -> Result<Vec<backup::BackupInfo>, String> {
    Ok(backup::list_backups(&PathBuf::from(install_dir)))
}

/// Restore a snapshot (Mods folder, or the whole install for a full backup).
/// Runs off the UI thread since a full restore can be large.
#[tauri::command]
async fn restore_backup(install_dir: String, id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || backup::restore_backup(&PathBuf::from(install_dir), &id))
        .await
        .map_err(|e| format!("restore task failed: {e}"))?
}

/// First-run defaults, resolved from the OS rather than hardcoded, so a fresh
/// install works on any machine (no baked-in username or VS Launcher paths):
/// - `installations_dir`: `%APPDATA%\Translocator\Installations` - our own
///   folder, created if missing so listing and Create both work immediately.
/// - `game_exe`: the base game the official Anego Studios installer drops at
///   `%APPDATA%\Vintagestory\Vintagestory.exe`. This is only a fallback - for
///   installs pinned to a downloaded version, `play` uses our version cache.
#[derive(Serialize)]
struct SuggestedPaths {
    installations_dir: String,
    game_exe: String,
}

#[tauri::command]
fn suggested_paths(app: AppHandle) -> SuggestedPaths {
    let roaming = app.path().data_dir().ok();
    let installations_dir = roaming
        .as_ref()
        .map(|r| r.join("Translocator").join("Installations"))
        .map(|p| {
            let _ = std::fs::create_dir_all(&p);
            p.to_string_lossy().into_owned()
        })
        .unwrap_or_default();
    let game_exe = roaming
        .as_ref()
        .map(|r| r.join("Vintagestory").join("Vintagestory.exe").to_string_lossy().into_owned())
        .unwrap_or_default();
    SuggestedPaths {
        installations_dir,
        game_exe,
    }
}

/// Installations folders belonging to other launchers (VS Launcher, StoryForge)
/// found on this machine, so the user can point at one and adopt in place.
#[tauri::command]
fn detect_launchers(app: AppHandle) -> Vec<migrate::DetectedLauncher> {
    migrate::detect(&app)
}

/// Seed translocator.json for un-adopted installs in `installations_dir` from the
/// managing launcher's config (version, params, playtime, ...). Non-destructive;
/// never touches game data or copies a session. Returns how many were enriched.
#[tauri::command]
fn import_from_launcher(app: AppHandle, installations_dir: String) -> migrate::ImportResult {
    migrate::import_from_launcher(&app, &PathBuf::from(installations_dir))
}

/// Every world in an installation's `Saves` folder, with parsed metadata (seed,
/// playstyle, height, versions) and filesystem facts. Reading the small
/// savegame blobs is fast even for large worlds, but runs off the UI thread so a
/// folder of many worlds never stutters.
#[tauri::command]
async fn list_worlds(install_dir: String) -> Result<Vec<worlds::WorldInfo>, String> {
    tokio::task::spawn_blocking(move || worlds::list_worlds(&PathBuf::from(install_dir)))
        .await
        .map_err(|e| format!("worlds task failed: {e}"))
}

/// Copy a single world into the install's backup folder; returns the copy's path.
#[tauri::command]
async fn backup_world(install_dir: String, world_path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        worlds::backup_world(&PathBuf::from(install_dir), &PathBuf::from(world_path))
    })
    .await
    .map_err(|e| format!("backup task failed: {e}"))?
}

/// Permanently delete a world (guarded in the UI by a confirm).
#[tauri::command]
fn delete_world(install_dir: String, world_path: String) -> Result<(), String> {
    worlds::delete_world(&PathBuf::from(install_dir), &PathBuf::from(world_path))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            login,
            get_account,
            logout,
            open_url,
            list_installations,
            save_installation,
            delete_installation,
            open_install_folder,
            create_installation,
            list_available_versions,
            list_cached_versions,
            ensure_version,
            remove_version,
            play,
            search_mods,
            install_mod,
            mod_donations,
            check_deps,
            check_updates,
            install_release,
            list_mod_files,
            backup_mods,
            backup_install,
            list_backups,
            restore_backup,
            list_worlds,
            backup_world,
            delete_world,
            suggested_paths,
            detect_launchers,
            import_from_launcher
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
