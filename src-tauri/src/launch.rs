//! Launching the game against a specific installation.
//!
//! `--dataPath=<install>` is the entire multi-install isolation mechanism: the
//! game reads/writes saves, config, mods, and clientsettings.json under that
//! directory. We wait for exit so the caller can then read back the session the
//! game wrote.

use std::path::Path;
use tokio::process::Command;

/// Spawn the game pointed at `install_dir` and wait for it to exit.
/// `start_params` are extra CLI args (whitespace-split, VS Launcher's format);
/// `env_vars` is a freeform "K=V, K2=V2" string. `extra_args` are passed through
/// verbatim, one arg each - use them for values that may contain spaces (e.g. a
/// `--pw` password) where whitespace-splitting would corrupt the value. Returns
/// the process exit code.
pub async fn launch_and_wait(
    game_exe: &str,
    install_dir: &Path,
    start_params: Option<&str>,
    env_vars: Option<&str>,
    extra_args: &[String],
) -> Result<i32, String> {
    let mut cmd = Command::new(game_exe);
    // The game resolves relative paths (notably the bundled "Mods" entry in
    // clientsettings modPaths) against its working directory. Without this,
    // the child inherits Translocator's own CWD and can pick up system mods
    // from a DIFFERENT install at a different game version, producing a
    // version-mixed client that no server will accept.
    if let Some(exe_dir) = Path::new(game_exe).parent() {
        cmd.current_dir(exe_dir);
    }
    cmd.arg(format!("--dataPath={}", install_dir.display()));
    if let Some(sp) = start_params {
        for part in sp.split_whitespace() {
            cmd.arg(part);
        }
    }
    for a in extra_args {
        cmd.arg(a);
    }
    if let Some(ev) = env_vars {
        for pair in ev.split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                let k = k.trim();
                if !k.is_empty() {
                    cmd.env(k, v.trim());
                }
            }
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch '{game_exe}': {e}"))?;
    let status = child
        .wait()
        .await
        .map_err(|e| format!("waiting on game process failed: {e}"))?;
    Ok(status.code().unwrap_or(-1))
}
