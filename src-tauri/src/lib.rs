//! Translocator - Tauri command surface.
//!
//! Crown jewel: log in once and launch any installation repeatedly WITHOUT
//! being bounced to re-login (where VS Launcher and StoryForge both fail) via
//! validate-before-stamp (`play` step 1) and read-back-after-exit (`play`
//! step 4). Also: persisted account (survives restart) and ModDB mod install.

mod auth;
mod backup;
mod curator;
mod deps;
mod hub;
mod manifest;
mod installations;
mod launch;
mod migrate;
mod models;
mod mods;
mod optimum;
mod pack_install;
mod servers;
mod session;
mod signing;
mod store;
mod updates;
mod versions;
mod worlds;

use models::{Account, AccountView};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// The login wall. Every command outside the pre-auth set (`login`,
/// `get_account`, `suggested_paths`) refuses until an account is stored, so a
/// hidden UI is never the only thing keeping a locked launcher locked. "Signed
/// in" means a stored session exists, NOT that the account server can be
/// reached right now.
const LOGIN_REQUIRED: &str = "Sign in with your Vintage Story account to use Translocator.";

/// The stored account, or the wall's error. For commands that need the account
/// itself.
fn require_account(app: &AppHandle) -> Result<Account, String> {
    store::load_account(app).ok_or_else(|| LOGIN_REQUIRED.to_string())
}

/// The same wall as a bare gate, for commands that only need to sit behind it.
fn require_login(app: &AppHandle) -> Result<(), String> {
    require_account(app).map(|_| ())
}

/// Result of a login attempt, tagged for the frontend.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum LoginOutcome {
    Success { account: AccountView },
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
        // Return only the non-sensitive view; the session key/signature stay in
        // the backend and on disk, never reaching the webview.
        Ok(LoginOutcome::Success { account: account.view() })
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

/// The persisted account as a non-sensitive view, if any (called on app startup
/// to restore the signed-in display state without exposing the session token).
#[tauri::command]
fn get_account(app: AppHandle) -> Option<AccountView> {
    store::load_account(&app).map(|a| a.view())
}

/// Open a URL in the user's default browser (e.g. a mod's ModDB page). Only web
/// URLs are allowed, so this can never be steered into launching a local file or
/// a custom scheme handler.
#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    require_login(&app)?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("refused to open a non-web URL".into());
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Forget the persisted account.
#[tauri::command]
fn logout(app: AppHandle) -> Result<(), String> {
    require_login(&app)?;
    store::clear_account(&app)
}

/// Every installation under `installations_dir`, with its metadata (adopting any
/// folder we don't yet track). `default_version` is used only when adopting a
/// folder that has no metadata yet.
#[tauri::command]
fn list_installations(
    app: AppHandle,
    installations_dir: String,
    default_version: String,
) -> Result<Vec<installations::InstallationCard>, String> {
    require_login(&app)?;
    installations::list(&PathBuf::from(installations_dir), &default_version)
}

/// Persist edited installation metadata (rename, params, env, backup prefs,
/// favorite, icon).
#[tauri::command]
fn save_installation(
    app: AppHandle,
    path: String,
    meta: installations::InstallationMeta,
) -> Result<(), String> {
    require_login(&app)?;
    installations::write_meta(&PathBuf::from(path), &meta)
}

/// Delete an installation folder. The UI confirms first.
#[tauri::command]
fn delete_installation(app: AppHandle, installations_dir: String, path: String) -> Result<(), String> {
    require_login(&app)?;
    let dir = PathBuf::from(&path);
    // Containment: only ever remove a real installation that lives directly
    // inside the configured installations folder, never an arbitrary path handed
    // to the command.
    if dir.parent() != Some(PathBuf::from(&installations_dir).as_path())
        || !installations::looks_like_install(&dir)
    {
        return Err("refused to delete: not an installation in the configured folder".into());
    }
    installations::delete(&dir)
}

/// Reveal an installation's folder in the OS file browser.
#[tauri::command]
fn open_install_folder(app: AppHandle, path: String) -> Result<(), String> {
    require_login(&app)?;
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
    require_login(&app)?;
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
    require_login(&app)?;
    versions::fetch_available(&app).await
}

/// Cached version strings (downloaded + installed).
#[tauri::command]
fn list_cached_versions(app: AppHandle) -> Result<Vec<String>, String> {
    require_login(&app)?;
    Ok(versions::list_cached(&app))
}

/// Ensure a version is in the cache (download + silent install if needed).
/// Emits "version-progress". Returns the cached exe path.
#[tauri::command]
async fn ensure_version(app: AppHandle, version: String, url: String, md5: String) -> Result<String, String> {
    require_login(&app)?;
    let exe = versions::ensure_version(&app, &version, &url, &md5).await?;
    // Optimization is a background sibling of the vanilla install, never a
    // dependency of it: this returns the vanilla exe immediately either way.
    optimum::maybe_autobuild(&app, &version);
    Ok(exe)
}

/// Remove a cached version's binaries, and the Optimum package built from them
/// (a full second copy of the game that has nothing left to optimize).
#[tauri::command]
fn remove_version(app: AppHandle, version: String) -> Result<(), String> {
    require_login(&app)?;
    optimum::remove_package(&app, &version);
    versions::remove_cached(&app, &version)
}

// ---- Optimum (optimized client packages, one per game version) ----

/// Everything the Settings panel needs for one version: consent, prerequisites,
/// and whether a package exists. Cheap enough to call on view changes.
#[tauri::command]
async fn optimum_status(app: AppHandle, version: Option<String>) -> Result<optimum::OptimumStatus, String> {
    require_login(&app)?;
    Ok(optimum::status(&app, version.as_deref()).await)
}

/// Optimum's end-user notice, read out of the release that would be built.
/// Downloading the release is not invoking it, so this runs before consent.
#[tauri::command]
async fn optimum_eula(app: AppHandle, version: String) -> Result<optimum::EulaText, String> {
    require_login(&app)?;
    optimum::eula(&app, &version).await
}

/// Record that the user accepted the notice for a given Optimum release. No
/// build ever runs before this exists.
#[tauri::command]
fn accept_optimum_eula(app: AppHandle, release: String) -> Result<(), String> {
    require_login(&app)?;
    optimum::accept_eula(&app, &release)
}

/// Turn the optimized launch path on or off. Stored backend-side because `play`
/// is what reads it.
#[tauri::command]
fn set_use_optimum(app: AppHandle, enabled: bool) -> Result<(), String> {
    require_login(&app)?;
    optimum::set_use_optimum(&app, enabled)
}

/// Install the missing build prerequisites into Translocator's own app data
/// (.NET 10 SDK, portable Git, ilspycmd). No admin, no system PATH changes.
#[tauri::command]
async fn provision_toolchain(app: AppHandle) -> Result<optimum::Prereqs, String> {
    require_login(&app)?;
    optimum::provision(&app).await
}

/// Build the optimized package for a version. Emits "optimum-progress".
///
/// This is the one place the login wall becomes a live check rather than a
/// stored-session check: a build is done on behalf of an account that owns the
/// game, so the account server must answer and answer yes.
#[tauri::command]
async fn optimize_version(app: AppHandle, version: String) -> Result<String, String> {
    let account = require_account(&app)?;
    match auth::client_validate(&account.uid, &account.sessionkey).await {
        Ok(v) if v.valid == 1 => {}
        Ok(v) => {
            return Err(format!(
                "Your Vintage Story session is no longer valid ({}). Sign in again to build the optimized client.",
                v.reason.unwrap_or_else(|| "nosession".into())
            ))
        }
        Err(e) => {
            return Err(format!(
                "Could not reach the Vintage Story account server, which is required before building the optimized client: {e}"
            ))
        }
    }
    optimum::optimize(&app, &version).await
}

/// Result of a play attempt.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum PlayResult {
    /// The stored session is dead server-side; the UI must re-login.
    NeedsRelogin { reason: String },
    /// The game ran. If `rotated`, the game issued a new session on exit; the
    /// refreshed session was persisted server-side and `account` is its view.
    Played {
        exit_code: i32,
        rotated: bool,
        account: AccountView,
    },
}

/// Launch the given installation and wait for it to exit. When `connect` is set,
/// the game is pointed at that server via `--connect` (plus `--pw` if a password
/// is given), which is how the server browser joins directly.
async fn play_inner(
    app: AppHandle,
    game_exe: String,
    install_dir: String,
    connect: Option<(String, Option<String>)>,
) -> Result<PlayResult, String> {
    let account = match store::load_account(&app) {
        Some(a) => a,
        None => return Ok(PlayResult::NeedsRelogin { reason: "not signed in".into() }),
    };
    // 1. Validate the stored session with the account server BEFORE trusting it.
    //    Two outcomes that must never be conflated, the same distinction the game
    //    itself draws between a Bad session and being Offline:
    //      - the server answered and refused us (valid != 1): the session is dead
    //        server-side and only a fresh login fixes it.
    //      - the call itself failed (Err = request or parse error): the account
    //        server is unreachable. Offline single player never needed it, so
    //        launch anyway with the stored session.
    //    Multiplayer is the exception: joining a server needs a live mptoken from
    //    this same auth server, so with a connect target unreachable stays a hard
    //    failure instead of a silent single-player launch.
    match auth::client_validate(&account.uid, &account.sessionkey).await {
        Ok(v) if v.valid == 1 => {}
        Ok(v) => {
            return Ok(PlayResult::NeedsRelogin {
                reason: v.reason.unwrap_or_else(|| "nosession".into()),
            });
        }
        Err(e) if connect.is_some() => {
            return Err(format!(
                "Could not reach the Vintage Story account server, which is required to join a multiplayer server: {e}"
            ));
        }
        Err(e) => {
            eprintln!("clientvalidate unreachable ({e}); launching offline with the stored session");
        }
    }

    let dir = PathBuf::from(&install_dir);
    let meta = installations::read_meta(&dir).unwrap_or_default();

    // Resolve the game binary: prefer our cached copy for the install's pinned
    // version; fall back to the caller-provided exe (e.g. an existing VS
    // Launcher binary) so already-set-up installs keep working.
    let vanilla_exe = if !meta.version.is_empty() {
        versions::exe_path(&app, &meta.version)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(game_exe)
    } else {
        game_exe
    };
    // Prefer the optimized package built for this version, if one exists and the
    // user hasn't opted out. Never the Vintagestory.exe INSIDE that package: its
    // Mods folder holds patched built-in mods that only Optimum's engine expects.
    let optimized = if !meta.version.is_empty() && optimum::enabled(&app) {
        optimum::package_exe(&app, &meta.version).map(|p| p.to_string_lossy().into_owned())
    } else {
        None
    };
    let exe = optimized.clone().unwrap_or_else(|| vanilla_exe.clone());
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

    // 3. Launch with the install's params/env (plus any --connect target) and
    //    wait; time the session.
    let mut extra: Vec<String> = Vec::new();
    if let Some((address, password)) = &connect {
        extra.push("--connect".into());
        extra.push(address.clone());
        if let Some(pw) = password.as_ref().filter(|p| !p.is_empty()) {
            extra.push("--pw".into());
            extra.push(pw.clone());
        }
    }
    let started = std::time::Instant::now();
    let mut exit_code = launch::launch_and_wait(
        &exe,
        &dir,
        Some(&meta.start_params),
        Some(&meta.env_vars),
        &extra,
    )
    .await?;
    let mut elapsed = started.elapsed().as_secs();

    // An optimized launch that dies immediately is a patch abort, not a play
    // session: Optimum restores its vanilla backups and exits without starting
    // the game, so there is nothing to salvage but plenty to fall back to.
    if optimized.is_some() {
        if let Some(reason) = optimum::should_fall_back(&dir, exit_code, elapsed) {
            eprintln!("optimized launch aborted for {}: {reason}", meta.version);
            optimum::mark_needs_rebuild(&app, &meta.version, &reason);
            optimum::notify_fallback(&app, &meta.version, &reason);
            let retry = std::time::Instant::now();
            exit_code = launch::launch_and_wait(
                &vanilla_exe,
                &dir,
                Some(&meta.start_params),
                Some(&meta.env_vars),
                &extra,
            )
            .await?;
            elapsed = retry.elapsed().as_secs();
        }
    }
    installations::record_play(&dir, elapsed);

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
        account: refreshed.view(),
    })
}

/// The crown jewel: validate -> stamp -> launch -> read-back. Launch params,
/// env vars, and auto-backup come from the installation's own metadata.
///
/// The account is loaded from the backend store here, not passed in from the
/// webview, so the session key/signature never make the round trip through the
/// frontend just to launch.
#[tauri::command]
async fn play(app: AppHandle, game_exe: String, install_dir: String) -> Result<PlayResult, String> {
    require_login(&app)?;
    play_inner(app, game_exe, install_dir, None).await
}

/// Launch an installation and connect straight to a server (the server browser's
/// Join). Same validate/stamp/read-back path as `play`, plus `--connect`.
#[tauri::command]
async fn connect_server(
    app: AppHandle,
    game_exe: String,
    install_dir: String,
    address: String,
    password: Option<String>,
) -> Result<PlayResult, String> {
    require_login(&app)?;
    play_inner(app, game_exe, install_dir, Some((address, password))).await
}

/// Add a server to an installation's in-game multiplayer list so it shows up in
/// the game's own server browser after launch (the "Add to installation" action).
#[tauri::command]
fn add_server_to_install(
    app: AppHandle,
    install_dir: String,
    name: String,
    address: String,
    password: Option<String>,
) -> Result<(), String> {
    require_login(&app)?;
    session::add_multiplayer_server(&PathBuf::from(install_dir), &name, &address, password.as_deref())
}

/// Text-search ModDB (most-downloaded first).
#[tauri::command]
async fn search_mods(app: AppHandle, text: String) -> Result<Vec<mods::ModSummary>, String> {
    require_login(&app)?;
    mods::search(&text).await
}

/// Download a mod's latest release into the install's Mods folder.
#[tauri::command]
async fn install_mod(app: AppHandle, install_dir: String, modidstr: String) -> Result<String, String> {
    require_login(&app)?;
    refuse_if_managed(&install_dir, true)?;
    mods::install_latest(&app, &PathBuf::from(install_dir), &modidstr).await
}

/// A mod's discovery detail for the Mods tab: ModDB description + recent
/// releases (with changelogs and game-version tags).
#[tauri::command]
async fn mod_detail(app: AppHandle, modidstr: String) -> Result<mods::ModDetailView, String> {
    require_login(&app)?;
    mods::mod_detail(&modidstr, 5).await
}

/// A mod's ModDB page URL, resolved live (the page route wants the assetid,
/// which manifests don't carry). The frontend opens it via open_url.
#[tauri::command]
async fn mod_page_url(app: AppHandle, modidstr: String) -> Result<String, String> {
    require_login(&app)?;
    mods::page_url(&modidstr).await
}

/// Author donation link(s) for a mod - the structured `donateurl` field if the
/// API exposes it, otherwise parsed from the description. Reference-only.
#[tauri::command]
async fn mod_donations(app: AppHandle, modidstr: String) -> Result<Vec<String>, String> {
    require_login(&app)?;
    mods::get_donations(&modidstr).await
}

/// The freeze, enforced backend-side: while an install is pack-managed, its
/// mod set belongs to the pack publisher. `blocking_extras` gates ADDING mods
/// (refused only for strict packs); leaving it false gates changing pack-owned
/// versions (refused for every managed install).
fn refuse_if_managed(install_dir: &str, blocking_extras: bool) -> Result<(), String> {
    let meta = match installations::read_meta(&PathBuf::from(install_dir)) {
        Some(m) => m,
        None => return Ok(()),
    };
    if meta.managed_by.is_empty() {
        return Ok(());
    }
    if blocking_extras && !meta.pack_strict {
        return Ok(()); // non-strict packs allow user-added mods
    }
    let what = if blocking_extras {
        "is a strict pack install; extra mods would break the server match"
    } else {
        "is managed by its pack; mod versions only change through a pack update"
    };
    Err(format!("This installation {what} ({}).", meta.managed_by))
}

/// Installed mods with newer ModDB releases, each release carrying its
/// compatibility (vs `game_version`) and changelog. Emits "check-progress".
#[tauri::command]
async fn check_updates(
    app: AppHandle,
    install_dir: String,
    game_version: String,
) -> Result<Vec<updates::ModUpdate>, String> {
    require_login(&app)?;
    refuse_if_managed(&install_dir, false)?;
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
    require_login(&app)?;
    refuse_if_managed(&install_dir, false)?;
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
fn check_deps(app: AppHandle, install_dir: String, filename: String) -> Result<Vec<deps::MissingDep>, String> {
    require_login(&app)?;
    let base = PathBuf::from(&install_dir);
    let zip = base.join("Mods").join(&filename);
    Ok(deps::check_missing_deps(&base, &zip))
}

/// Zip filenames currently in an install's Mods folder.
#[tauri::command]
fn list_mod_files(app: AppHandle, install_dir: String) -> Result<Vec<String>, String> {
    require_login(&app)?;
    Ok(mods::list_mod_files(&PathBuf::from(install_dir)))
}

/// Snapshot the install's Mods folder into a fresh timestamped backup; returns
/// the backup id. Called before applying mod updates (fast, mods-only). Runs off
/// the UI thread.
#[tauri::command]
async fn backup_mods(app: AppHandle, install_dir: String) -> Result<String, String> {
    require_login(&app)?;
    tokio::task::spawn_blocking(move || backup::backup_mods(&PathBuf::from(install_dir)))
        .await
        .map_err(|e| format!("backup task failed: {e}"))?
}

/// Snapshot the WHOLE installation (worlds, config, mods) into a compressed
/// backup at the given level, pruned to `keep`. Heavy (gigabytes), so it runs on
/// a blocking thread to keep the UI responsive. Returns the backup id.
#[tauri::command]
async fn backup_install(app: AppHandle, install_dir: String, compression: u8, keep: u8) -> Result<String, String> {
    require_login(&app)?;
    tokio::task::spawn_blocking(move || {
        backup::backup_install(&PathBuf::from(install_dir), compression, keep as usize)
    })
    .await
    .map_err(|e| format!("backup task failed: {e}"))?
}

/// All backups for an install, newest-first.
#[tauri::command]
fn list_backups(app: AppHandle, install_dir: String) -> Result<Vec<backup::BackupInfo>, String> {
    require_login(&app)?;
    Ok(backup::list_backups(&PathBuf::from(install_dir)))
}

/// Restore a snapshot (Mods folder, or the whole install for a full backup).
/// Runs off the UI thread since a full restore can be large.
#[tauri::command]
async fn restore_backup(app: AppHandle, install_dir: String, id: String) -> Result<(), String> {
    require_login(&app)?;
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

/// Build a modpack manifest from an installation: resolve every mod against
/// ModDB, pin it with a local SHA-256, and report the mods that aren't on ModDB.
/// Publishing is a separate, explicit step. `min_launcher_version` defaults to
/// this launcher's version when the caller leaves it blank.
#[tauri::command]
async fn curate_pack(
    app: AppHandle,
    install_dir: String,
    mut pack: manifest::ManifestPack,
    server: Option<manifest::ManifestServer>,
    links: Option<manifest::ManifestLinks>,
    override_paths: Vec<String>,
) -> Result<curator::CuratorPreview, String> {
    require_login(&app)?;
    if pack.min_launcher_version.trim().is_empty() {
        pack.min_launcher_version = env!("CARGO_PKG_VERSION").to_string();
    }
    curator::build_manifest(&PathBuf::from(install_dir), pack, links, server, override_paths).await
}

// ---- Publisher identity: the VS account itself. ----
// No tokens, no operator-maintained keys. A DPAPI-sealed Ed25519 device key
// signs on behalf of the signed-in VS account; the Hub binds key to account
// once, via a single-use mptoken validated by VS's own auth server.

#[derive(serde::Serialize)]
struct PublisherStatus {
    signed_in: bool,
    playername: Option<String>,
    has_key: bool,
    fingerprint: Option<String>,
}

/// What the curator UI needs to render the publish section: who we'd publish
/// as, and whether this machine has a signing key yet. Never creates a key.
#[tauri::command]
fn publisher_status(app: AppHandle) -> Result<PublisherStatus, String> {
    require_login(&app)?;
    // The pre-VS-account bearer token is obsolete; scrub it if this machine
    // still has one from the earlier flow.
    let _ = store::remove_sealed(&app, "publisher.dat");
    let account = store::load_account(&app);
    let has_key = signing::has_key(&app);
    let fingerprint = if has_key {
        signing::load_or_create_key(&app).ok().map(|k| signing::fingerprint(&k))
    } else {
        None
    };
    Ok(PublisherStatus {
        signed_in: account.is_some(),
        playername: account.map(|a| a.playername),
        has_key,
        fingerprint,
    })
}

/// Register this device's signing key to the signed-in VS account:
/// Hub challenge -> single-use mptoken from VS's auth server (using the stored
/// session, backend-side only) -> Hub verifies with VS and binds the key.
/// Idempotent; safe to call before every first publish from a machine.
#[tauri::command]
async fn register_publisher(app: AppHandle, hub_url: String) -> Result<serde_json::Value, String> {
    let account = require_account(&app)?;
    let key = signing::load_or_create_key(&app)?;
    let base = hub_url.trim_end_matches('/').to_string();
    let client = reqwest::Client::new();

    let challenge: serde_json::Value = client
        .post(format!("{base}/api/publishers/challenge"))
        .send()
        .await
        .map_err(|e| format!("could not reach the Hub: {e}"))?
        .json()
        .await
        .map_err(|e| format!("challenge parse failed: {e}"))?;
    let nonce = challenge
        .get("serverlogintoken")
        .and_then(|v| v.as_str())
        .ok_or("Hub sent no challenge")?
        .to_string();

    let mptoken = auth::request_mptoken(&account.uid, &account.sessionkey, &nonce).await?;

    let resp = client
        .post(format!("{base}/api/publishers/register"))
        .json(&serde_json::json!({
            "uid": account.uid,
            "mptokenv2": mptoken,
            "serverlogintoken": nonce,
            "public_key": signing::public_key_b64(&key),
        }))
        .send()
        .await
        .map_err(|e| format!("register request failed: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_else(|_| serde_json::json!({}));
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("registration rejected ({status}): {body}"))
    }
}

/// Sign and publish a built manifest to the Hub as the signed-in VS account.
/// The manifest is serialized to JSON once; that exact value feeds the payload
/// digest AND the wire envelope, so the Hub canonicalizes identical bytes.
#[tauri::command]
async fn publish_pack(app: AppHandle, hub_url: String, manifest: manifest::Manifest) -> Result<String, String> {
    let account = require_account(&app)?;
    let key = signing::load_or_create_key(&app)?;
    let value = serde_json::to_value(&manifest).map_err(|e| format!("manifest serialize failed: {e}"))?;
    let payload = signing::signing_payload(&value, &account.uid)?;
    let signature = signing::sign(&key, &payload);
    curator::publish(&hub_url, &manifest.pack.id, &value, &signature, &account.uid).await
}

/// `ModConfig/*.json` files in an install, as override candidates for the curator.
#[tauri::command]
fn list_config_files(app: AppHandle, install_dir: String) -> Result<Vec<String>, String> {
    require_login(&app)?;
    Ok(curator::list_config_files(&PathBuf::from(install_dir)))
}

/// Browse published packs on the Hub (the Market list).
#[tauri::command]
async fn hub_list_packs(app: AppHandle, hub_url: String) -> Result<Vec<hub::PackSummary>, String> {
    require_login(&app)?;
    hub::list_packs(&hub_url).await
}

/// Full record for one pack (the Pack page header).
#[tauri::command]
async fn hub_pack(app: AppHandle, hub_url: String, id: String) -> Result<serde_json::Value, String> {
    require_login(&app)?;
    hub::pack_detail(&hub_url, &id).await
}

/// A pack's manifest (latest, or a pinned version), for its mod + override list.
#[tauri::command]
async fn hub_pack_manifest(
    app: AppHandle,
    hub_url: String,
    id: String,
    version: Option<String>,
) -> Result<manifest::Manifest, String> {
    require_login(&app)?;
    hub::pack_manifest(&hub_url, &id, version.as_deref()).await
}

/// Install a published pack as a fresh pack-managed installation: fetch the
/// latest manifest, gate on min_launcher_version, ensure the game version,
/// stage-download + verify every pinned mod, place, apply overrides, add the
/// pack's server, and freeze. Emits "pack-progress". Returns the install path.
#[tauri::command]
async fn install_pack(
    app: AppHandle,
    installations_dir: String,
    hub_url: String,
    pack_id: String,
    name: String,
    seed_from: Option<String>,
    optional_choices: Option<std::collections::BTreeMap<String, bool>>,
) -> Result<String, String> {
    require_login(&app)?;
    pack_install::install_pack(
        &app,
        &PathBuf::from(installations_dir),
        &hub_url,
        &pack_id,
        &name,
        seed_from,
        optional_choices.unwrap_or_default(),
    )
    .await
}

/// The Vintage Story masterserver public server list (the Servers > Public tab).
#[tauri::command]
async fn list_public_servers(app: AppHandle) -> Result<Vec<servers::PublicServer>, String> {
    require_login(&app)?;
    servers::list_public_servers().await
}

/// The user's saved private servers (the Servers > Private tab).
#[tauri::command]
fn list_private_servers(app: AppHandle) -> Result<Vec<servers::PrivateServer>, String> {
    require_login(&app)?;
    Ok(servers::list_private(&app))
}

/// Add or update a saved private server; returns the refreshed list.
#[tauri::command]
fn save_private_server(app: AppHandle, server: servers::PrivateServer) -> Result<Vec<servers::PrivateServer>, String> {
    require_login(&app)?;
    servers::upsert_private(&app, server)
}

/// Remove a saved private server by id; returns the refreshed list.
#[tauri::command]
fn remove_private_server(app: AppHandle, id: String) -> Result<Vec<servers::PrivateServer>, String> {
    require_login(&app)?;
    servers::remove_private(&app, &id)
}

/// Installations folders belonging to other launchers (VS Launcher, StoryForge)
/// found on this machine, so the user can point at one and adopt in place.
#[tauri::command]
fn detect_launchers(app: AppHandle) -> Result<Vec<migrate::DetectedLauncher>, String> {
    require_login(&app)?;
    Ok(migrate::detect(&app))
}

/// Seed translocator.json for un-adopted installs in `installations_dir` from the
/// managing launcher's config (version, params, playtime, ...). Non-destructive;
/// never touches game data or copies a session. Returns how many were enriched.
#[tauri::command]
fn import_from_launcher(
    app: AppHandle,
    installations_dir: String,
) -> Result<migrate::ImportResult, String> {
    require_login(&app)?;
    Ok(migrate::import_from_launcher(&app, &PathBuf::from(installations_dir)))
}

/// Every world in an installation's `Saves` folder, with parsed metadata (seed,
/// playstyle, height, versions) and filesystem facts. Reading the small
/// savegame blobs is fast even for large worlds, but runs off the UI thread so a
/// folder of many worlds never stutters.
#[tauri::command]
async fn list_worlds(app: AppHandle, install_dir: String) -> Result<Vec<worlds::WorldInfo>, String> {
    require_login(&app)?;
    tokio::task::spawn_blocking(move || worlds::list_worlds(&PathBuf::from(install_dir)))
        .await
        .map_err(|e| format!("worlds task failed: {e}"))
}

/// Copy a single world into the install's backup folder; returns the copy's path.
#[tauri::command]
async fn backup_world(app: AppHandle, install_dir: String, world_path: String) -> Result<String, String> {
    require_login(&app)?;
    tokio::task::spawn_blocking(move || {
        worlds::backup_world(&PathBuf::from(install_dir), &PathBuf::from(world_path))
    })
    .await
    .map_err(|e| format!("backup task failed: {e}"))?
}

/// Permanently delete a world (guarded in the UI by a confirm).
#[tauri::command]
fn delete_world(app: AppHandle, install_dir: String, world_path: String) -> Result<(), String> {
    require_login(&app)?;
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
            optimum_status,
            optimum_eula,
            accept_optimum_eula,
            set_use_optimum,
            provision_toolchain,
            optimize_version,
            play,
            search_mods,
            install_mod,
            mod_donations,
            mod_page_url,
            mod_detail,
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
            import_from_launcher,
            curate_pack,
            publish_pack,
            list_config_files,
            publisher_status,
            register_publisher,
            hub_list_packs,
            hub_pack,
            hub_pack_manifest,
            install_pack,
            list_public_servers,
            connect_server,
            add_server_to_install,
            list_private_servers,
            save_private_server,
            remove_private_server
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
