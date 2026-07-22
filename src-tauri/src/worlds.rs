//! Worlds browser: reads each installation's `Saves/*.vcdbs` (VS savegames).
//!
//! A `.vcdbs` is a SQLite database; its `gamedata` table holds a single row with
//! one ProtoBuf.NET-serialized `SaveGame` blob. We open that database read-only
//! (immutable, so a running game's WAL never blocks us) and pull the top-level
//! scalar fields VS writes for every world: name, seed, playstyle, world height,
//! game version, and last-saved time.
//!
//! Parsing is best-effort. We only read protobuf fields whose tag we recognise
//! and keep the first occurrence of each; anything unknown (including the
//! multi-megabyte chunk/block blobs) is skipped by index math, never decoded, so
//! listing a 150 MB world costs about as much as reading its 3 MB metadata blob.
//! If a world can't be opened or decoded it still lists with its filesystem
//! facts (name from the filename, size, modified time) - the browser never hides
//! a save just because its metadata didn't parse.

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const BACKUP_DIR: &str = ".translocator-backups";

#[derive(Serialize, Default)]
pub struct WorldInfo {
    /// Absolute path to the `.vcdbs`.
    pub path: String,
    pub filename: String,
    /// In-world name from the savegame; falls back to the filename stem.
    pub name: String,
    /// Signed 32-bit worldgen seed (VS displays it signed).
    pub seed: Option<i64>,
    /// Playstyle langcode, e.g. `surviveandbuild`, `creativebuilding`.
    pub playstyle: String,
    /// Vertical world size in blocks (256 default, 384 tall, ...).
    pub world_height: Option<u32>,
    /// Game version the world was created on.
    pub created_version: String,
    /// Game version the world was last loaded on.
    pub last_version: String,
    /// ISO timestamp of the last save, from the savegame itself (may be empty).
    pub last_played: String,
    pub size_bytes: u64,
    /// Filesystem mtime in ms since epoch (used for sorting; always present).
    pub modified_ms: u64,
    /// True when the savegame blob decoded; false = filesystem facts only.
    pub parsed: bool,
}

#[derive(Default)]
struct SaveMeta {
    name: String,
    seed: Option<i64>,
    playstyle: String,
    height: Option<u32>,
    created_version: String,
    last_version: String,
    last_played: String,
}

/// Read a protobuf base-128 varint, advancing `i`. Returns None on truncation.
fn read_varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut shift = 0u32;
    let mut result = 0u64;
    loop {
        let byte = *b.get(*i)?;
        *i += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None; // malformed
        }
    }
}

/// Walk the top level of the `SaveGame` protobuf, capturing the first value of
/// each field tag we care about. Only small string fields are decoded; large and
/// unknown payloads are skipped by advancing the cursor.
fn parse_savegame(b: &[u8]) -> SaveMeta {
    let mut m = SaveMeta::default();
    let mut i = 0usize;
    while i < b.len() {
        let key = match read_varint(b, &mut i) {
            Some(k) => k,
            None => break,
        };
        let field = key >> 3;
        let wire = key & 7;
        match wire {
            0 => {
                // varint scalar
                let v = match read_varint(b, &mut i) {
                    Some(v) => v,
                    None => break,
                };
                match field {
                    2 if m.height.is_none() => m.height = Some(v as u32),
                    7 if m.seed.is_none() => m.seed = Some(v as i64), // sign-correct
                    _ => {}
                }
            }
            2 => {
                // length-delimited
                let len = match read_varint(b, &mut i) {
                    Some(l) => l as usize,
                    None => break,
                };
                let end = match i.checked_add(len) {
                    Some(e) if e <= b.len() => e,
                    _ => break,
                };
                // Only decode the short string metadata fields; skip the rest.
                if len < 256 {
                    let slot = match field {
                        13 => Some(&mut m.name),
                        17 => Some(&mut m.last_played),
                        18 => Some(&mut m.created_version),
                        21 => Some(&mut m.last_version),
                        29 => Some(&mut m.playstyle),
                        _ => None,
                    };
                    if let Some(slot) = slot {
                        if slot.is_empty() {
                            if let Ok(s) = std::str::from_utf8(&b[i..end]) {
                                *slot = s.to_string();
                            }
                        }
                    }
                }
                i = end;
            }
            5 => i += 4, // fixed32
            1 => i += 8, // fixed64
            _ => break,  // group/unknown wire type - stop, keep what we have
        }
    }
    m
}

/// Build a SQLite `file:` URI with `immutable=1` so we can read a savegame even
/// while the game holds it open, without touching or requiring its WAL.
fn immutable_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut enc = String::from(if raw.starts_with('/') { "file://" } else { "file:///" });
    for c in raw.chars() {
        match c {
            ' ' => enc.push_str("%20"),
            '#' => enc.push_str("%23"),
            '%' => enc.push_str("%25"),
            '?' => enc.push_str("%3f"),
            _ => enc.push(c),
        }
    }
    enc.push_str("?immutable=1");
    enc
}

fn read_savegame_meta(path: &Path) -> Option<SaveMeta> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(immutable_uri(path), flags).ok()?;
    let blob: Vec<u8> = conn
        .query_row("SELECT data FROM gamedata LIMIT 1", [], |r| r.get(0))
        .ok()?;
    Some(parse_savegame(&blob))
}

fn modified_ms(md: &std::fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Every `.vcdbs` under `<install>/Saves`, newest-first by modified time.
pub fn list_worlds(install_dir: &Path) -> Vec<WorldInfo> {
    let saves = install_dir.join("Saves");
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(&saves) {
        Ok(rd) => rd,
        Err(_) => return out, // no Saves folder yet
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file()
            || path.extension().map(|e| !e.eq_ignore_ascii_case("vcdbs")).unwrap_or(true)
        {
            continue;
        }
        let md = entry.metadata().ok();
        let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let mut w = WorldInfo {
            path: path.to_string_lossy().into_owned(),
            filename,
            name: stem,
            size_bytes: md.as_ref().map(|m| m.len()).unwrap_or(0),
            modified_ms: md.as_ref().map(modified_ms).unwrap_or(0),
            ..Default::default()
        };
        if let Some(meta) = read_savegame_meta(&path) {
            w.parsed = true;
            if !meta.name.is_empty() {
                w.name = meta.name;
            }
            w.seed = meta.seed;
            w.playstyle = meta.playstyle;
            w.world_height = meta.height;
            w.created_version = meta.created_version;
            w.last_version = meta.last_version;
            w.last_played = meta.last_played;
        }
        out.push(w);
    }
    out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    out
}

/// A `.vcdbs` path is inside the install's `Saves` folder (guards the
/// destructive commands against being pointed anywhere else).
fn is_world_in(install_dir: &Path, world_path: &Path) -> bool {
    let saves = install_dir.join("Saves");
    world_path.parent() == Some(saves.as_path())
        && world_path.extension().map(|e| e.eq_ignore_ascii_case("vcdbs")).unwrap_or(false)
}

/// Copy a single world into `<install>/.translocator-backups/worlds/` with a
/// sortable timestamp, so it can be recovered before a risky edit or deletion.
/// Returns the backup file's absolute path.
pub fn backup_world(install_dir: &Path, world_path: &Path) -> Result<String, String> {
    let world_path = PathBuf::from(world_path);
    if !is_world_in(install_dir, &world_path) {
        return Err("that file is not a world in this installation".into());
    }
    if !world_path.is_file() {
        return Err("world file not found".into());
    }
    let stem = world_path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let dest_dir = install_dir.join(BACKUP_DIR).join("worlds");
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("create backup dir failed: {e}"))?;
    let dest = dest_dir.join(format!("{stem}__{}.vcdbs", crate::backup::new_id()));
    std::fs::copy(&world_path, &dest).map_err(|e| format!("copy failed: {e}"))?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Delete a world outright (the `.vcdbs` plus any `-wal`/`-shm`/journal
/// sidecars). The UI guards this with a confirm; back up first if unsure.
pub fn delete_world(install_dir: &Path, world_path: &Path) -> Result<(), String> {
    let world_path = PathBuf::from(world_path);
    if !is_world_in(install_dir, &world_path) {
        return Err("that file is not a world in this installation".into());
    }
    std::fs::remove_file(&world_path).map_err(|e| format!("delete failed: {e}"))?;
    // Best-effort sidecar cleanup; absence is fine.
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut side = world_path.clone().into_os_string();
        side.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(side));
    }
    Ok(())
}
