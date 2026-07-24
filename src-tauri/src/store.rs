//! Persisting the logged-in account across restarts.
//!
//! At rest the account is ENCRYPTED. On Windows we wrap the JSON with DPAPI
//! (`CryptProtectData`), which ties the ciphertext to the current Windows user
//! account: another user (or the file copied to another machine) cannot decrypt
//! it, and the plaintext session token never touches the disk. The sealed blob
//! lives in `account.dat`.
//!
//! A pre-existing plaintext `account.json` (from before this change, or on a
//! non-Windows dev build) is migrated to the sealed form on first load and then
//! deleted. On non-Windows targets there is no DPAPI, so the bytes are stored
//! as-is; the app is Windows-first and this path only exists so it still builds.

use crate::models::Account;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const SEALED_FILE: &str = "account.dat";
const LEGACY_PLAINTEXT: &str = "account.json";

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn save_account(app: &AppHandle, account: &Account) -> Result<(), String> {
    let json = serde_json::to_vec(account).map_err(|e| e.to_string())?;
    let sealed = seal(&json)?;
    let dir = data_dir(app)?;
    std::fs::write(dir.join(SEALED_FILE), sealed).map_err(|e| e.to_string())?;
    // Never leave a plaintext copy behind.
    let legacy = dir.join(LEGACY_PLAINTEXT);
    if legacy.exists() {
        let _ = std::fs::remove_file(legacy);
    }
    Ok(())
}

pub fn load_account(app: &AppHandle) -> Option<Account> {
    let dir = data_dir(app).ok()?;

    let sealed_path = dir.join(SEALED_FILE);
    if sealed_path.is_file() {
        let sealed = std::fs::read(&sealed_path).ok()?;
        let json = unseal(&sealed).ok()?;
        return serde_json::from_slice(&json).ok();
    }

    // Migrate a legacy plaintext account to the sealed form, then drop it.
    let legacy = dir.join(LEGACY_PLAINTEXT);
    if legacy.is_file() {
        let text = std::fs::read_to_string(&legacy).ok()?;
        let account: Account = serde_json::from_str(&text).ok()?;
        let _ = save_account(app, &account); // re-seals and removes the plaintext
        return Some(account);
    }
    None
}

/// Write arbitrary bytes to a DPAPI-sealed file in the app data dir. Used for
/// any at-rest secret beyond the account (e.g. saved-server passwords).
pub fn save_sealed(app: &AppHandle, filename: &str, bytes: &[u8]) -> Result<(), String> {
    let sealed = seal(bytes)?;
    let dir = data_dir(app)?;
    std::fs::write(dir.join(filename), sealed).map_err(|e| e.to_string())
}

/// Read + unseal a file written by `save_sealed`. `None` if absent/undecryptable.
pub fn load_sealed(app: &AppHandle, filename: &str) -> Option<Vec<u8>> {
    let dir = data_dir(app).ok()?;
    let p = dir.join(filename);
    if !p.is_file() {
        return None;
    }
    let sealed = std::fs::read(&p).ok()?;
    unseal(&sealed).ok()
}

/// Delete a sealed file. No-op if it doesn't exist.
pub fn remove_sealed(app: &AppHandle, filename: &str) -> Result<(), String> {
    let p = data_dir(app)?.join(filename);
    if p.exists() {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn clear_account(app: &AppHandle) -> Result<(), String> {
    let dir = data_dir(app)?;
    for name in [SEALED_FILE, LEGACY_PLAINTEXT] {
        let p = dir.join(name);
        if p.exists() {
            std::fs::remove_file(p).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn seal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    dpapi::protect(bytes)
}
#[cfg(windows)]
fn unseal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    dpapi::unprotect(bytes)
}
#[cfg(not(windows))]
fn seal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(bytes.to_vec())
}
#[cfg(not(windows))]
fn unseal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(bytes.to_vec())
}

#[cfg(windows)]
mod dpapi {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    // Never show a UI prompt; fail instead.
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    fn in_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            // DPAPI treats the input as read-only; the cast is sound.
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// Copy the DPAPI output out of the OS-allocated buffer, then LocalFree it.
    unsafe fn take_out(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let owned = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as *mut core::ffi::c_void);
        owned
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = in_blob(data);
            let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
            let ok = CryptProtectData(
                &input,
                ptr::null(), // description
                ptr::null(), // no extra entropy
                ptr::null(), // reserved
                ptr::null(), // no prompt struct
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );
            if ok == 0 {
                return Err("CryptProtectData failed".into());
            }
            Ok(take_out(output))
        }
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = in_blob(data);
            let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
            let ok = CryptUnprotectData(
                &input,
                ptr::null_mut(), // description out (unused)
                ptr::null(),     // entropy
                ptr::null(),     // reserved
                ptr::null(),     // no prompt struct
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );
            if ok == 0 {
                return Err("CryptUnprotectData failed".into());
            }
            Ok(take_out(output))
        }
    }
}
