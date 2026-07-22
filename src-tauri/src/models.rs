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
