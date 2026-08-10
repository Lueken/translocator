//! Optimum integration: build one optimized client package per game version and
//! launch it instead of the vanilla exe.
//!
//! Optimum (by Zaldaryon, GPL-3.0 + Commons Clause) transforms game binaries,
//! not player data, so the artifact is per VERSION, not per installation:
//!
//! ```text
//! %APPDATA%\Translocator\
//!   Versions\<ver>\           vanilla cache (Optimum only ever reads it)
//!   Optimum\<ver>\Optimum\    the built package (the installer adds that leaf)
//!   toolchain\                user-local dotnet + git + ilspycmd, no admin
//!   OptimumSrc\<tag>\         extracted release, deleted after a build
//! ```
//!
//! Three rules this module exists to keep:
//!
//! 1. **Orchestration only.** We download Zaldaryon's own GitHub release on the
//!    user's machine and run its installer as a child process. No Optimum code,
//!    text, or script is vendored into this repo - even the EULA below is read
//!    out of the downloaded script at runtime rather than copied here.
//! 2. **Consent is never swallowed by automation.** The silent installer skips
//!    the GUI licence dialog, so nothing builds until the user has accepted the
//!    notice we surfaced from that same script.
//! 3. **Nothing here may ever block installing or playing.** Every failure
//!    degrades to vanilla with a note.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Optimum releases pin themselves to one game version (plus explicit bridge
/// patch sets). This is the known-good mapping; anything not listed falls back
/// to reading the newest release's own `forks.json`, which is the real answer.
const KNOWN_RELEASES: &[(&str, &str)] = &[("1.22.5", "0.3.5"), ("1.22.6", "0.3.5")];

/// ilspycmd is version-gated by the installer (`.config/ilspycmd-compat.json`
/// accepts 10.1.0.8386 - 10.1.1.8388 as of Optimum 0.3.5), so provisioning pins
/// the low end of that range rather than taking whatever is newest on NuGet.
const ILSPYCMD_VERSION: &str = "10.1.0.8386";

const DOTNET_INSTALL_SCRIPT: &str = "https://dot.net/v1/dotnet-install.ps1";
const GIT_RELEASES_API: &str = "https://api.github.com/repos/git-for-windows/git/releases/latest";
const OPTIMUM_RELEASES_API: &str = "https://api.github.com/repos/Zaldaryon/Optimum/releases";
const USER_AGENT: &str = "Translocator";

// ---------------------------------------------------------------- paths ----

fn translocator_root(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .data_dir()
        .map_err(|e| format!("no data dir: {e}"))?
        .join("Translocator");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn version_dir(app: &AppHandle, version: &str) -> Result<PathBuf, String> {
    Ok(translocator_root(app)?.join("Versions").join(version))
}

/// Where we ask the installer to install.
fn package_root(app: &AppHandle, version: &str) -> Result<PathBuf, String> {
    Ok(translocator_root(app)?.join("Optimum").join(version))
}

/// The built client, if the package exists. This is the ground truth for
/// "ready" - no state file can go stale against it. The installer's layout
/// differs by mode (verified live 2026-08-09): -Silent installs DIRECTLY into
/// -InstallDir, while the GUI appends its own `Optimum` leaf to the chosen
/// folder. Probe both so packages from either path count as ready.
pub fn package_exe(app: &AppHandle, version: &str) -> Option<PathBuf> {
    let root = package_root(app, version).ok()?;
    for p in [root.join("Optimum.exe"), root.join("Optimum").join("Optimum.exe")] {
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn toolchain_root(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = translocator_root(app)?.join("toolchain");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn log_path(app: &AppHandle, version: &str) -> Result<PathBuf, String> {
    let dir = translocator_root(app)?.join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("optimum-install-{version}.log")))
}

/// Delete a version's package. Called when its vanilla binaries are removed, so
/// a full second copy of the game never outlives the version it was built from.
pub fn remove_package(app: &AppHandle, version: &str) {
    if let Ok(p) = package_root(app, version) {
        if p.exists() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

// ---------------------------------------------------------------- state ----

#[derive(Serialize, Deserialize, Clone)]
struct OptimumState {
    use_optimum: bool,
    /// The release tag whose notice the user accepted, e.g. "0.3.5".
    eula_accepted_release: Option<String>,
    /// version -> last failure reason.
    failures: BTreeMap<String, String>,
    /// version -> newest release tag checked that did not support it.
    no_release: BTreeMap<String, String>,
}

impl Default for OptimumState {
    fn default() -> Self {
        Self {
            use_optimum: true,
            eula_accepted_release: None,
            failures: BTreeMap::new(),
            no_release: BTreeMap::new(),
        }
    }
}

/// Consent and preferences, not secrets, so this is plain JSON next to the
/// sealed account rather than a DPAPI blob.
fn state_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| format!("no app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("optimum.json"))
}

fn load_state(app: &AppHandle) -> OptimumState {
    state_file(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_state(app: &AppHandle, state: &OptimumState) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(state_file(app)?, json).map_err(|e| e.to_string())
}

/// Versions with a build in flight. Also the serialization point: Optimum takes
/// a per-package lock of its own, and two builds of one version would race the
/// same InstallDir.
fn building() -> &'static Mutex<HashSet<String>> {
    static B: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_building(version: &str) -> bool {
    building().lock().map(|b| b.contains(version)).unwrap_or(false)
}

/// Claim the build slot for a version. `false` means someone else holds it.
fn claim_build(version: &str) -> bool {
    match building().lock() {
        Ok(mut b) => b.insert(version.to_string()),
        Err(_) => false,
    }
}

fn release_build(version: &str) {
    if let Ok(mut b) = building().lock() {
        b.remove(version);
    }
}

/// Whether launches should prefer a built package. The frontend toggle writes
/// this; `play` reads it, so the backend never depends on the webview's copy.
pub fn enabled(app: &AppHandle) -> bool {
    load_state(app).use_optimum
}

// -------------------------------------------------------------- prereqs ----

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Our provisioned tool directories, in the order the child should see them.
fn prefix_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let Ok(root) = toolchain_root(app) else { return Vec::new() };
    vec![root.join("dotnet"), root.join("git").join("cmd"), root.join("tools")]
}

/// PATH for Optimum child processes: our prefixes first, then the user's own.
/// We never write to the system PATH or the registry, so provisioning leaves
/// nothing behind outside Translocator's app data.
fn child_path(app: &AppHandle) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut parts: Vec<String> = prefix_dirs(app)
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(&sep.to_string())
}

/// Find an executable across our prefixes and the inherited PATH.
fn find_tool(app: &AppHandle, stem: &str) -> Option<PathBuf> {
    let name = exe_name(stem);
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in child_path(app).split(sep) {
        if dir.trim().is_empty() {
            continue;
        }
        let p = Path::new(dir.trim()).join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[derive(Serialize, Clone)]
pub struct Prereqs {
    pub dotnet: bool,
    pub git: bool,
    pub ilspycmd: bool,
}

impl Prereqs {
    fn all_ok(&self) -> bool {
        self.dotnet && self.git && self.ilspycmd
    }
}

/// Probe the four things the installer needs (Windows PowerShell 5.1 is part of
/// the OS, so it is not probed). The .NET check mirrors the installer's own:
/// a `dotnet` on PATH is not enough, it must list a 10.x SDK.
pub async fn detect_prereqs(app: &AppHandle) -> Prereqs {
    let dotnet = match find_tool(app, "dotnet") {
        Some(exe) => {
            let out = tokio::process::Command::new(exe)
                .arg("--list-sdks")
                .env("PATH", child_path(app))
                .output()
                .await;
            match out {
                Ok(o) => String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.trim_start().starts_with("10.")),
                Err(_) => false,
            }
        }
        None => false,
    };
    Prereqs {
        dotnet,
        git: find_tool(app, "git").is_some(),
        ilspycmd: find_tool(app, "ilspycmd").is_some(),
    }
}

// --------------------------------------------------------------- status ----

#[derive(Serialize)]
pub struct OptimumStatus {
    pub use_optimum: bool,
    pub eula_accepted: bool,
    pub eula_release: Option<String>,
    pub prereqs: Prereqs,
    /// none | building | ready | failed | unsupported
    pub package_state: String,
    pub package_path: Option<String>,
    /// Last failure reason, or why the version is unsupported.
    pub detail: Option<String>,
    /// The release tag we would build from, when we already know it.
    pub release: Option<String>,
}

pub async fn status(app: &AppHandle, version: Option<&str>) -> OptimumStatus {
    let state = load_state(app);
    let prereqs = detect_prereqs(app).await;

    let (package_state, package_path, detail, release) = match version {
        Some(v) if !v.is_empty() => {
            let known = KNOWN_RELEASES
                .iter()
                .find(|(ver, _)| *ver == v)
                .map(|(_, tag)| tag.to_string());
            if let Some(p) = package_exe(app, v) {
                ("ready", Some(p.to_string_lossy().into_owned()), None, known)
            } else if is_building(v) {
                ("building", None, None, known)
            } else if let Some(tag) = state.no_release.get(v) {
                (
                    "unsupported",
                    None,
                    Some(format!(
                        "No Optimum release supports Vintage Story {v} yet (checked against {tag}). Translocator will optimize automatically when one lands."
                    )),
                    known,
                )
            } else if let Some(reason) = state.failures.get(v) {
                ("failed", None, Some(reason.clone()), known)
            } else {
                ("none", None, None, known)
            }
        }
        _ => ("none", None, None, None),
    };

    OptimumStatus {
        use_optimum: state.use_optimum,
        eula_accepted: state.eula_accepted_release.is_some(),
        eula_release: state.eula_accepted_release,
        prereqs,
        package_state: package_state.to_string(),
        package_path,
        detail,
        release,
    }
}

pub fn set_use_optimum(app: &AppHandle, on: bool) -> Result<(), String> {
    let mut state = load_state(app);
    state.use_optimum = on;
    save_state(app, &state)
}

pub fn accept_eula(app: &AppHandle, release: &str) -> Result<(), String> {
    let mut state = load_state(app);
    state.eula_accepted_release = Some(release.to_string());
    save_state(app, &state)
}

fn record_failure(app: &AppHandle, version: &str, reason: &str) {
    let mut state = load_state(app);
    state.failures.insert(version.to_string(), reason.to_string());
    let _ = save_state(app, &state);
}

fn clear_failure(app: &AppHandle, version: &str) {
    let mut state = load_state(app);
    if state.failures.remove(version).is_some() || state.no_release.remove(version).is_some() {
        let _ = save_state(app, &state);
    }
}

// ------------------------------------------------------------- progress ----

fn emit(app: &AppHandle, version: &str, phase: &str, detail: &str) {
    let _ = app.emit(
        "optimum-progress",
        serde_json::json!({ "version": version, "phase": phase, "detail": detail }),
    );
}

// --------------------------------------------------------- provisioning ----

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_default()
}

async fn download(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!("refused a non-HTTPS download: {url}"));
    }
    let resp = http()
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed ({url}): {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed ({url}): HTTP {}", resp.status()));
    }
    Ok(resp
        .bytes()
        .await
        .map_err(|e| format!("download read failed ({url}): {e}"))?
        .to_vec())
}

/// Extract a zip into `dest`, path-traversal safe via `enclosed_name`.
fn unzip(bytes: &[u8], dest: &Path) -> Result<(), String> {
    if bytes.len() < 4 || &bytes[..2] != b"PK" {
        return Err("the download was not a zip archive".into());
    }
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("open zip failed: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut of = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut of).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Install whichever prerequisites are missing into `toolchain\`, user-local and
/// without admin rights. Sources are Microsoft's own dotnet-install script, the
/// git-for-windows GitHub release, and NuGet via `dotnet tool install`; nothing
/// is mirrored through Lueken infrastructure.
pub async fn provision(app: &AppHandle) -> Result<Prereqs, String> {
    #[cfg(not(windows))]
    {
        let _ = app;
        return Err("Toolchain provisioning is Windows-only for now.".into());
    }

    #[cfg(windows)]
    {
        let root = toolchain_root(app)?;
        let mut have = detect_prereqs(app).await;

        if !have.dotnet {
            emit(app, "", "toolchain", "Downloading the .NET installer script from Microsoft...");
            let script = download(DOTNET_INSTALL_SCRIPT).await?;
            let text = String::from_utf8_lossy(&script);
            // Sanity-check the payload before executing it: a captive-portal or
            // error page must never reach PowerShell.
            if script.len() < 5_000 || !text.contains("dotnet-install") {
                return Err("the .NET install script did not look like Microsoft's script; refused to run it".into());
            }
            let script_path = root.join("dotnet-install.ps1");
            std::fs::write(&script_path, &script).map_err(|e| e.to_string())?;

            emit(app, "", "toolchain", "Installing the .NET 10 SDK (a few hundred MB)...");
            let out = tokio::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    &script_path.to_string_lossy(),
                    "-Channel",
                    "10.0",
                    "-InstallDir",
                    &root.join("dotnet").to_string_lossy(),
                ])
                .output()
                .await
                .map_err(|e| format!("could not run the .NET installer: {e}"))?;
            let _ = std::fs::remove_file(&script_path);
            if !out.status.success() {
                return Err(format!(
                    ".NET SDK install failed: {}",
                    last_lines(&String::from_utf8_lossy(&out.stderr), 3)
                ));
            }
        }

        if !have.git {
            emit(app, "", "toolchain", "Finding the portable Git release...");
            let body = download(GIT_RELEASES_API).await?;
            let json: serde_json::Value =
                serde_json::from_slice(&body).map_err(|e| format!("git release parse failed: {e}"))?;
            let url = json
                .get("assets")
                .and_then(|a| a.as_array())
                .and_then(|assets| {
                    assets.iter().find(|a| {
                        let n = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        n.starts_with("MinGit-") && n.ends_with("-64-bit.zip") && !n.contains("busybox")
                    })
                })
                .and_then(|a| a.get("browser_download_url"))
                .and_then(|v| v.as_str())
                .ok_or("no MinGit 64-bit asset in the latest git-for-windows release")?
                .to_string();

            emit(app, "", "toolchain", "Downloading portable Git...");
            let zip = download(&url).await?;
            if zip.len() < 1_000_000 {
                return Err("the MinGit download was too small to be the real archive".into());
            }
            let git_dir = root.join("git");
            let _ = std::fs::remove_dir_all(&git_dir);
            std::fs::create_dir_all(&git_dir).map_err(|e| e.to_string())?;
            unzip(&zip, &git_dir)?;
            if !git_dir.join("cmd").join("git.exe").is_file() {
                return Err("portable Git extracted but cmd\\git.exe is missing".into());
            }
        }

        if !have.ilspycmd {
            // Needs dotnet, which may have only just landed in our prefix.
            let dotnet = find_tool(app, "dotnet")
                .ok_or("the .NET SDK is required before ilspycmd can be installed")?;
            emit(app, "", "toolchain", "Installing ilspycmd...");
            let out = tokio::process::Command::new(dotnet)
                .args([
                    "tool",
                    "install",
                    "ilspycmd",
                    "--tool-path",
                    &root.join("tools").to_string_lossy(),
                    "--version",
                    ILSPYCMD_VERSION,
                ])
                .env("PATH", child_path(app))
                .env("DOTNET_ROOT", root.join("dotnet"))
                .output()
                .await
                .map_err(|e| format!("could not run dotnet tool install: {e}"))?;
            if !out.status.success() {
                let msg = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                return Err(format!("ilspycmd install failed: {}", last_lines(&msg, 3)));
            }
        }

        have = detect_prereqs(app).await;
        emit(app, "", "toolchain-done", "Toolchain ready.");
        Ok(have)
    }
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" / ")
}

// ------------------------------------------------------ release handling ----

struct Release {
    tag: String,
    url: String,
}

/// The release we should build `version` from. Prefers the known mapping; when
/// a version is not in it, the newest release is fetched and its own manifest
/// decides (see `release_supports`), which is the forward path as Optimum ships
/// new versions.
async fn resolve_release(version: &str) -> Result<Release, String> {
    let body = download(OPTIMUM_RELEASES_API).await?;
    let list: Vec<serde_json::Value> =
        serde_json::from_slice(&body).map_err(|e| format!("Optimum release list parse failed: {e}"))?;
    if list.is_empty() {
        return Err("the Optimum repository has no releases".into());
    }

    let wanted = KNOWN_RELEASES
        .iter()
        .find(|(ver, _)| *ver == version)
        .map(|(_, tag)| *tag);

    let pick = wanted
        .and_then(|tag| {
            list.iter().find(|r| {
                r.get("tag_name")
                    .and_then(|v| v.as_str())
                    .map(|t| t.trim_start_matches('v') == tag)
                    .unwrap_or(false)
            })
        })
        .or_else(|| list.iter().find(|r| !r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false)))
        .ok_or("no usable Optimum release found")?;

    let tag = pick
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .trim_start_matches('v')
        .to_string();

    // Releases are source-only. Prefer an uploaded zip asset, fall back to the
    // GitHub source zipball. Either way the bytes come from Zaldaryon's repo.
    let asset = pick
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| {
                    a.get("name")
                        .and_then(|v| v.as_str())
                        .map(|n| n.ends_with(".zip"))
                        .unwrap_or(false)
                })
                .and_then(|a| a.get("browser_download_url"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string())
        .or_else(|| {
            pick.get("zipball_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .ok_or("the Optimum release had no downloadable archive")?;

    Ok(Release { tag, url: asset })
}

/// The extracted release root: the folder holding `install-windows.cmd`. GitHub
/// source zips wrap everything in one `<repo>-<sha>` directory.
fn find_release_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("install-windows.cmd").is_file() {
        return Some(dir.to_path_buf());
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() && p.join("install-windows.cmd").is_file() {
            return Some(p);
        }
    }
    None
}

fn forks_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("forks.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("vintageStoryVersion")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// A release supports a game version if `forks.json` pins it, or it ships a
/// `patches-<ver>-bridge/` set. Same rule the installer applies to itself.
fn release_supports(root: &Path, version: &str) -> bool {
    forks_version(root).as_deref() == Some(version)
        || root.join(format!("patches-{version}-bridge")).is_dir()
}

/// Optimum's END-USER NOTICE, read out of the downloaded installer script so
/// the text the user accepts is the text that release actually ships.
fn extract_eula(root: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(root.join("scripts").join("install-windows.ps1"))
        .map_err(|e| format!("could not read the Optimum installer: {e}"))?;
    let start = text
        .find("END-USER NOTICE")
        .ok_or("the Optimum installer did not contain its end-user notice")?;
    let rest = &text[start..];
    let end = rest
        .find("\n\"@")
        .ok_or("the Optimum installer's end-user notice was not terminated as expected")?;
    Ok(rest[..end].trim().to_string())
}

/// Download + extract a release into `OptimumSrc\<tag>`, reusing it if present.
async fn stage_release(app: &AppHandle, version: &str, release: &Release) -> Result<PathBuf, String> {
    let work = translocator_root(app)?.join("OptimumSrc").join(&release.tag);
    if let Some(root) = find_release_root(&work) {
        return Ok(root);
    }
    emit(app, version, "download", &format!("Downloading Optimum {}...", release.tag));
    let bytes = download(&release.url).await?;
    if bytes.len() < 100_000 {
        return Err("the Optimum release archive was too small to be valid".into());
    }
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    emit(app, version, "extract", "Extracting the release...");
    unzip(&bytes, &work)?;
    find_release_root(&work).ok_or_else(|| "the Optimum archive had no install-windows.cmd".into())
}

#[derive(Serialize)]
pub struct EulaText {
    pub release: String,
    pub text: String,
}

/// The notice for the release that would be used for `version`. Staging the
/// release is a download, not an invocation, so this is safe to call before
/// consent exists.
pub async fn eula(app: &AppHandle, version: &str) -> Result<EulaText, String> {
    let release = resolve_release(version).await?;
    let root = stage_release(app, version, &release).await?;
    Ok(EulaText {
        release: release.tag,
        text: extract_eula(&root)?,
    })
}

// ---------------------------------------------------------------- build ----

/// Build the optimized package for `version`. Long-running (decompile, five
/// clones, a full solution build), so callers should treat it as background
/// work. Returns the built exe path.
pub async fn optimize(app: &AppHandle, version: &str) -> Result<String, String> {
    if let Some(p) = package_exe(app, version) {
        return Ok(p.to_string_lossy().into_owned());
    }
    #[cfg(not(windows))]
    {
        return Err("Optimum builds are Windows-only for now.".into());
    }

    #[cfg(windows)]
    {
        let vs_path = version_dir(app, version)?;
        if !vs_path.join("Vintagestory.exe").is_file() {
            return Err(format!("Vintage Story {version} is not downloaded yet."));
        }
        let state = load_state(app);
        if state.eula_accepted_release.is_none() {
            return Err("Optimum's end-user notice has not been accepted yet.".into());
        }
        let prereqs = detect_prereqs(app).await;
        if !prereqs.all_ok() {
            return Err("The build toolchain is incomplete. Install it from Settings first.".into());
        }
        if !claim_build(version) {
            return Err(format!("An optimization for {version} is already running."));
        }

        let result = build_inner(app, version, &vs_path).await;
        release_build(version);

        match &result {
            Ok(_) => clear_failure(app, version),
            Err(e) => {
                record_failure(app, version, e);
                emit(app, version, "failed", e);
            }
        }
        result
    }
}

#[cfg(windows)]
async fn build_inner(app: &AppHandle, version: &str, vs_path: &Path) -> Result<String, String> {
    emit(app, version, "resolve", "Looking up a matching Optimum release...");
    let release = resolve_release(version).await?;
    let root = stage_release(app, version, &release).await?;

    if !release_supports(&root, version) {
        let mut state = load_state(app);
        state.no_release.insert(version.to_string(), release.tag.clone());
        let _ = save_state(app, &state);
        let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
        return Err(format!(
            "Optimum {} does not support Vintage Story {version} yet. Installations stay vanilla; Translocator will retry when a matching release appears.",
            release.tag
        ));
    }

    let install_dir = package_root(app, version)?;
    std::fs::create_dir_all(&install_dir).map_err(|e| e.to_string())?;
    let log = log_path(app, version)?;
    let _ = std::fs::remove_file(&log);

    let mut args: Vec<String> = vec![
        "-Silent".into(),
        "-VsPath".into(),
        vs_path.to_string_lossy().into_owned(),
        "-InstallDir".into(),
        install_dir.to_string_lossy().into_owned(),
        "-LogFile".into(),
        log.to_string_lossy().into_owned(),
    ];
    // Only needed for a bridge build, where the release pins a different game
    // version than the one we are building against.
    if forks_version(&root).as_deref() != Some(version) {
        args.push("-Version".into());
        args.push(version.to_string());
    }

    emit(
        app,
        version,
        "build",
        &format!("Building the optimized client with Optimum {} (this takes several minutes)...", release.tag),
    );

    // Tail the installer's own log so the UI shows real phases rather than a
    // spinner for the whole build.
    let stop = Arc::new(AtomicBool::new(false));
    let tail = {
        let (app, log, stop, version) = (app.clone(), log.clone(), stop.clone(), version.to_string());
        tokio::spawn(async move {
            let mut last = String::new();
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(1200)).await;
                if let Ok(text) = std::fs::read_to_string(&log) {
                    if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
                        if line != last {
                            last = line.to_string();
                            emit(&app, &version, "build", &last);
                        }
                    }
                }
            }
        })
    };

    let cmd_path = root.join("install-windows.cmd");
    let status = run_installer(app, &cmd_path, &root, &args).await;
    stop.store(true, Ordering::Relaxed);
    tail.abort();
    let status = status?;

    if !status.success() {
        return Err(format!(
            "The Optimum installer exited with {status}. See {}",
            log.display()
        ));
    }
    match package_exe(app, version) {
        Some(p) => {
            // The release tree is several hundred MB of sources; it has done its
            // job. (The installer cleans its own build root under C:\opt-bld or
            // %LOCALAPPDATA%\opt-bld in a finally block.)
            if let Some(parent) = root.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
            emit(app, version, "done", "Optimized client ready.");
            Ok(p.to_string_lossy().into_owned())
        }
        None => Err(format!(
            "The Optimum installer reported success but Optimum.exe is missing. See {}",
            log.display()
        )),
    }
}

/// Run the release's own entry point. Rust can execute a `.cmd` directly, but if
/// this host refuses to, fall back to exactly what that wrapper does in its
/// silent branch rather than failing the build.
#[cfg(windows)]
async fn run_installer(
    app: &AppHandle,
    cmd_path: &Path,
    root: &Path,
    args: &[String],
) -> Result<std::process::ExitStatus, String> {
    let dotnet_root = toolchain_root(app)?.join("dotnet");
    let build = |program: &Path, full: Vec<String>| {
        let mut c = tokio::process::Command::new(program);
        c.args(full)
            .current_dir(root)
            .env("PATH", child_path(app));
        if dotnet_root.is_dir() {
            c.env("DOTNET_ROOT", &dotnet_root);
        }
        c
    };

    match build(cmd_path, args.to_vec()).status().await {
        Ok(s) => Ok(s),
        Err(_) => {
            let ps1 = root.join("scripts").join("install-windows.ps1");
            let mut full = vec![
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                ps1.to_string_lossy().into_owned(),
            ];
            full.extend(args.iter().cloned());
            build(Path::new("powershell.exe"), full)
                .status()
                .await
                .map_err(|e| format!("could not run the Optimum installer: {e}"))
        }
    }
}

/// Called after a vanilla version lands in the cache. Kicks off a background
/// build when everything is already consented and provisioned; otherwise does
/// nothing at all. Never awaited by the caller: downloading a version must not
/// wait on a multi-minute build.
pub fn maybe_autobuild(app: &AppHandle, version: &str) {
    let state = load_state(app);
    if !state.use_optimum || state.eula_accepted_release.is_none() {
        return;
    }
    if package_exe(app, version).is_some() || is_building(version) {
        return;
    }
    if state.no_release.contains_key(version) || state.failures.contains_key(version) {
        return;
    }
    let (app, version) = (app.clone(), version.to_string());
    tauri::async_runtime::spawn(async move {
        if !detect_prereqs(&app).await.all_ok() {
            return;
        }
        if let Err(e) = optimize(&app, &version).await {
            eprintln!("background optimization for {version} skipped: {e}");
        }
    });
}

// --------------------------------------------------------------- launch ----

/// A nonzero exit this fast is a patch abort, not a play session: Optimum
/// restores its vanilla mod backups and exits without ever starting the game.
const FAST_FAIL_SECS: u64 = 20;

/// Whether an optimized launch that just exited should be retried as vanilla.
/// Optimum has no distinct abort exit code yet, so the signal is a fast nonzero
/// exit, corroborated by its launcher log when one exists.
pub fn should_fall_back(install_dir: &Path, exit_code: i32, elapsed_secs: u64) -> Option<String> {
    if exit_code == 0 || elapsed_secs > FAST_FAIL_SECS {
        return None;
    }
    let log = install_dir.join("Logs").join("optimum-launcher.log");
    let detail = std::fs::read_to_string(&log)
        .ok()
        .filter(|t| t.contains("did not produce a complete patched runtime"))
        .map(|_| "Optimum could not patch this version's runtime")
        .unwrap_or("The optimized client exited immediately");
    Some(format!("{detail}; relaunching the vanilla client."))
}

/// Mark a version as needing a rebuild after its package failed at launch.
pub fn mark_needs_rebuild(app: &AppHandle, version: &str, reason: &str) {
    record_failure(app, version, reason);
}

pub fn notify_fallback(app: &AppHandle, version: &str, reason: &str) {
    emit(app, version, "fallback", reason);
}
