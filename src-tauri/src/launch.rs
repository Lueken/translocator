//! Launching the game against a specific installation.
//!
//! `--dataPath=<install>` is the entire multi-install isolation mechanism: the
//! game reads/writes saves, config, mods, and clientsettings.json under that
//! directory. We wait for exit so the caller can then read back the session the
//! game wrote.

use std::path::Path;
use tokio::process::Command;

/// Spawn the game pointed at `install_dir` and wait for it to exit.
/// Returns the process exit code.
pub async fn launch_and_wait(
    game_exe: &str,
    install_dir: &Path,
    start_params: Option<&str>,
) -> Result<i32, String> {
    let mut cmd = Command::new(game_exe);
    cmd.arg(format!("--dataPath={}", install_dir.display()));
    if let Some(sp) = start_params {
        for part in sp.split_whitespace() {
            cmd.arg(part);
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
