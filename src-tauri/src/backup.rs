//! Snapshot/restore of an installation's `Mods/` folder, so a mod update can be
//! rolled back. A backup is just a copy of every `*.zip` in `<install>/Mods/`
//! into `<install>/.translocator-backups/<id>/`, where `<id>` is a sortable
//! timestamp. `backup_mods` is called before applying updates; `restore_backup`
//! swaps the live Mods zips back to a snapshot.
//!
//! Design notes:
//! - The backup root `.translocator-backups` lives inside the install but is
//!   excluded from every Mods scan (it sits beside Mods, not within it).
//! - An empty Mods folder still produces an (empty) backup entry so restore is
//!   predictable - restoring it clears the Mods folder, which is the honest
//!   inverse of "there was nothing installed".
//! - Restore is guarded against path traversal: `id` must be a simple dir name.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const BACKUP_DIR: &str = ".translocator-backups";
const DEFAULT_KEEP: usize = 5;
/// Top-level entries never included in a whole-install backup.
const EXCLUDE_TOP: &[&str] = &[BACKUP_DIR, "Cache", "Logs"];
/// Top-level files never included (our own metadata; restoring it would revert
/// name/version/playtime to snapshot time).
const EXCLUDE_TOP_FILE: &[&str] = &["translocator.json"];

#[derive(Serialize)]
pub struct BackupInfo {
    pub id: String,
    /// "mods" (fast Mods-only snapshot) or "full" (whole compressed install).
    pub kind: String,
    pub mod_count: usize,
    /// Bytes on disk.
    pub size: u64,
    pub created: String,
}

/// `<install>/.translocator-backups`
fn backups_root(install_dir: &Path) -> PathBuf {
    install_dir.join(BACKUP_DIR)
}

/// Zip filenames in `<install>/Mods` (never touches the backup root, which lives
/// beside Mods rather than inside it).
fn mod_zips(install_dir: &Path) -> Vec<PathBuf> {
    let mods_dir = install_dir.join("Mods");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&mods_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

/// A backup `<id>` is safe to use as a directory name: non-empty, no path
/// separators, no parent refs. Matches the ids we mint in `new_id`.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Milliseconds since the Unix epoch (monotonic enough for ordering backups).
fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Convert a Unix timestamp (seconds) to a UTC `(year, month, day, hh, mm, ss)`
/// tuple using Howard Hinnant's civil-from-days algorithm.
fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = ((rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hh, mm, ss)
}

/// Sortable, filesystem-safe backup id like `20260721-153045-123` (UTC, ms).
/// Shared with the worlds browser so world backups sort alongside install ones.
pub(crate) fn new_id() -> String {
    let ms = epoch_millis();
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let (y, mo, d, hh, mm, ss) = civil_from_epoch(secs);
    format!("{y:04}{mo:02}{d:02}-{hh:02}{mm:02}{ss:02}-{millis:03}")
}

/// Human-readable UTC timestamp for display, e.g. `2026-07-21 15:30:45 UTC`.
fn created_from_id_millis(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let (y, mo, d, hh, mm, ss) = civil_from_epoch(secs);
    format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

/// Snapshot every `*.zip` in `<install>/Mods` into a fresh timestamped backup
/// dir and return its id. Creates an empty entry when nothing is installed.
/// Prunes older backups past the default keep count at the end.
pub fn backup_mods(install_dir: &Path) -> Result<String, String> {
    let id = new_id();
    let dest = backups_root(install_dir).join(&id);
    std::fs::create_dir_all(&dest).map_err(|e| format!("create backup dir failed: {e}"))?;

    for src in mod_zips(install_dir) {
        if let Some(name) = src.file_name() {
            std::fs::copy(&src, dest.join(name))
                .map_err(|e| format!("copy {} failed: {e}", name.to_string_lossy()))?;
        }
    }

    // Best-effort prune; a failure here shouldn't fail the backup itself.
    let _ = prune_backups(install_dir, DEFAULT_KEEP);
    Ok(id)
}

/// Snapshot the ENTIRE installation (worlds, config, mods, playerdata, ...) into
/// a single compressed `<id>.zip`, excluding transient/large dirs (Cache, Logs)
/// and our own metadata. `compression` is the deflate level 0-9. Prunes full
/// backups past `keep`. Returns the id.
pub fn backup_install(install_dir: &Path, compression: u8, keep: usize) -> Result<String, String> {
    let id = new_id();
    let root = backups_root(install_dir);
    std::fs::create_dir_all(&root).map_err(|e| format!("create backup dir failed: {e}"))?;
    // Write to a `.part` and rename on success, so an interrupted backup never
    // leaves a corrupt `<id>.zip` (list_backups only sees complete `.zip` files).
    let tmp = root.join(format!("{id}.zip.part"));
    let final_path = root.join(format!("{id}.zip"));

    let file = std::fs::File::create(&tmp).map_err(|e| format!("create backup zip failed: {e}"))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(compression.min(9) as i64));

    if let Err(e) = zip_dir_recursive(&mut zw, install_dir, install_dir, opts) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = zw.finish() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("finalize backup zip failed: {e}"));
    }
    std::fs::rename(&tmp, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("place backup zip failed: {e}")
    })?;

    let _ = prune_full(install_dir, keep.max(1));
    Ok(id)
}

fn zip_dir_recursive<W: std::io::Write + std::io::Seek>(
    zw: &mut zip::ZipWriter<W>,
    base: &Path,
    dir: &Path,
    opts: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let is_top = dir == base;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_top && (EXCLUDE_TOP.contains(&name.as_str()) || EXCLUDE_TOP_FILE.contains(&name.as_str())) {
            continue;
        }
        let path = e.path();
        let ft = match e.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            zip_dir_recursive(zw, base, &path, opts)?;
        } else if ft.is_file() {
            let rel = path.strip_prefix(base).map_err(|e| e.to_string())?;
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            zw.start_file(rel_str, opts).map_err(|e| format!("zip entry failed: {e}"))?;
            let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, zw).map_err(|e| format!("zip write failed: {e}"))?;
        }
    }
    Ok(())
}

fn count_zips_in_dir(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .filter(|f| f.path().extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                total += if m.is_dir() { dir_size(&e.path()) } else { m.len() };
            }
        }
    }
    total
}

/// All backups for an install, newest-first - both fast Mods-only snapshots
/// (`<id>/` dirs) and whole-install compressed snapshots (`<id>.zip`).
pub fn list_backups(install_dir: &Path) -> Vec<BackupInfo> {
    let root = backups_root(install_dir);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let fname = e.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                if !is_safe_id(&fname) {
                    continue;
                }
                let created = created_at(&e.path()).map(created_from_id_millis).unwrap_or_else(|| fname.clone());
                out.push(BackupInfo {
                    kind: "mods".into(),
                    mod_count: count_zips_in_dir(&e.path()),
                    size: dir_size(&e.path()),
                    created,
                    id: fname,
                });
            } else if ft.is_file() {
                if let Some(stem) = fname.strip_suffix(".zip") {
                    if !is_safe_id(stem) {
                        continue;
                    }
                    let size = std::fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0);
                    let created = created_at(&e.path()).map(created_from_id_millis).unwrap_or_else(|| stem.to_string());
                    out.push(BackupInfo { id: stem.to_string(), kind: "full".into(), mod_count: 0, size, created });
                }
            }
        }
    }
    // Ids are lexically sortable; newest-first (kinds interleave by time).
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// Filesystem creation/modified time of a backup dir as epoch millis, for the
/// human-readable `created` string. Falls back to modified time.
fn created_at(dir: &Path) -> Option<u128> {
    let meta = std::fs::metadata(dir).ok()?;
    let t = meta.created().or_else(|_| meta.modified()).ok()?;
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis())
}

/// Restore a snapshot: delete every `*.zip` in `<install>/Mods`, then copy the
/// backup's zips back. Guards against path traversal via `is_safe_id`.
pub fn restore_backup(install_dir: &Path, id: &str) -> Result<(), String> {
    if !is_safe_id(id) {
        return Err(format!("invalid backup id: {id}"));
    }
    // A full (whole-install) backup is a `<id>.zip`; extract it over the install.
    let zip_path = backups_root(install_dir).join(format!("{id}.zip"));
    if zip_path.is_file() {
        return restore_full(install_dir, &zip_path);
    }
    let src_dir = backups_root(install_dir).join(id);
    if !src_dir.is_dir() {
        return Err(format!("backup {id} not found"));
    }
    // Keep it inside the backup root even after resolving - belt and suspenders.
    let root = backups_root(install_dir);
    if !src_dir.starts_with(&root) {
        return Err(format!("invalid backup path for id: {id}"));
    }

    let mods_dir = install_dir.join("Mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| format!("create Mods dir failed: {e}"))?;

    // Clear current zips.
    for zip in mod_zips(install_dir) {
        std::fs::remove_file(&zip)
            .map_err(|e| format!("remove {} failed: {e}", zip.to_string_lossy()))?;
    }

    // Copy snapshot zips back.
    if let Ok(rd) = std::fs::read_dir(&src_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                if let Some(name) = p.file_name() {
                    std::fs::copy(&p, mods_dir.join(name))
                        .map_err(|e| format!("restore {} failed: {e}", name.to_string_lossy()))?;
                }
            }
        }
    }
    Ok(())
}

/// Extract a whole-install `<id>.zip` over the installation folder (path-traversal
/// safe via `enclosed_name`). Overwrites files present in the snapshot; files
/// added since the snapshot are left in place (a merge restore, so a mid-restore
/// failure never empties the install).
fn restore_full(install_dir: &Path, zip_path: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("open backup failed: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = install_dir.join(rel);
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

/// Delete the oldest full (`<id>.zip`) backups beyond `keep`.
pub fn prune_full(install_dir: &Path, keep: usize) -> Result<(), String> {
    let root = backups_root(install_dir);
    let mut ids: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let n = e.file_name().to_string_lossy().into_owned();
                if let Some(stem) = n.strip_suffix(".zip") {
                    if is_safe_id(stem) {
                        ids.push(stem.to_string());
                    }
                }
            }
        }
    }
    ids.sort_by(|a, b| b.cmp(a));
    for id in ids.into_iter().skip(keep) {
        let _ = std::fs::remove_file(root.join(format!("{id}.zip")));
    }
    Ok(())
}

/// Delete the oldest backup dirs beyond `keep`. No-op when at or under the cap.
pub fn prune_backups(install_dir: &Path, keep: usize) -> Result<(), String> {
    let root = backups_root(install_dir);
    let mut ids: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let id = e.file_name().to_string_lossy().into_owned();
                if is_safe_id(&id) {
                    ids.push(id);
                }
            }
        }
    }
    // Newest-first, then drop everything past `keep` (the oldest).
    ids.sort_by(|a, b| b.cmp(a));
    for id in ids.into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(root.join(id));
    }
    Ok(())
}
