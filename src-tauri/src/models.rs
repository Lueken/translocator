//! Shared account/session model.
//!
//! An `Account` is one Vintage Story login. The pair (`sessionkey`,
//! `sessionsignature`) is the identity the game RSA-verifies locally and the
//! account server validates. VS rotates this pair on every fresh login, which
//! is the whole reason the carryover in the existing launchers breaks - see
//! the brief, section 4.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Account {
    pub uid: String,
    pub playername: String,
    pub email: String,
    pub sessionkey: String,
    pub sessionsignature: String,
    #[serde(default)]
    pub mptoken: Option<String>,
    #[serde(default)]
    pub entitlements: Option<serde_json::Value>,
}

/// The non-sensitive subset of an account the UI is allowed to see. The session
/// key/signature, mptoken, and uid never cross into the webview: they stay in the
/// Rust backend and on disk, and are only ever written into a game's
/// clientsettings.json at launch. Every command that faces the frontend returns
/// this view, never a full `Account`.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub playername: String,
    pub email: String,
    #[serde(default)]
    pub entitlements: Option<serde_json::Value>,
}

impl Account {
    pub fn view(&self) -> AccountView {
        AccountView {
            playername: self.playername.clone(),
            email: self.email.clone(),
            entitlements: self.entitlements.clone(),
        }
    }
}
