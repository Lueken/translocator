//! Vintage Story account-server calls.
//!
//! Endpoints reverse-engineered from the decompiled game (`SessionManager.cs`)
//! and confirmed against both existing launchers:
//!   POST /v2/gamelogin   -> exchange email/password (+2FA) for a session
//!   POST /clientvalidate -> ask the server whether a session is still live
//!
//! `/clientvalidate` is the piece both incumbents skip. It cannot reissue a
//! rotated key (only `/v2/gamelogin` can), but it tells us BEFORE launch whether
//! the stored session is dead, so we never stamp a corpse into an install.

use serde::Deserialize;

const AUTH_BASE: &str = "https://auth3.vintagestory.at";
// Sent verbatim by the game; the server is lenient but expects the field.
const GAME_LOGIN_VERSION: &str = "1.22.0";

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    #[serde(default)]
    pub valid: u8,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub prelogintoken: Option<String>,
    #[serde(default)]
    pub sessionkey: Option<String>,
    #[serde(default)]
    pub sessionsignature: Option<String>,
    #[serde(default)]
    pub mptoken: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub playername: Option<String>,
    #[serde(default)]
    pub entitlements: Option<serde_json::Value>,
    #[serde(default)]
    pub hasgameserver: Option<bool>,
}

/// POST /v2/gamelogin. On first attempt for a 2FA account the server returns
/// `valid=0` plus a `prelogintoken`; resend with the TOTP code + that token.
pub async fn game_login(
    email: &str,
    password: &str,
    totp: Option<&str>,
    prelogin: Option<&str>,
) -> Result<LoginResponse, String> {
    let params = [
        ("email", email),
        ("password", password),
        ("totpcode", totp.unwrap_or("")),
        ("prelogintoken", prelogin.unwrap_or("")),
        ("gameloginversion", GAME_LOGIN_VERSION),
    ];
    let resp = reqwest::Client::new()
        .post(format!("{AUTH_BASE}/v2/gamelogin"))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("gamelogin request failed: {e}"))?;
    resp.json::<LoginResponse>()
        .await
        .map_err(|e| format!("gamelogin parse failed: {e}"))
}

#[derive(Debug, Deserialize)]
pub struct ValidateResponse {
    #[serde(default)]
    pub valid: u8,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub entitlements: Option<serde_json::Value>,
    #[serde(default)]
    pub mptoken: Option<String>,
    #[serde(default)]
    pub hasgameserver: Option<bool>,
}

/// POST /clientvalidate. `valid == 1` means the account server still recognizes
/// this session; anything else (typically reason "nosession") means it has been
/// rotated out and the user must log in again.
pub async fn client_validate(uid: &str, sessionkey: &str) -> Result<ValidateResponse, String> {
    let params = [("uid", uid), ("sessionkey", sessionkey)];
    let resp = reqwest::Client::new()
        .post(format!("{AUTH_BASE}/clientvalidate"))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("clientvalidate request failed: {e}"))?;
    resp.json::<ValidateResponse>()
        .await
        .map_err(|e| format!("clientvalidate parse failed: {e}"))
}

#[derive(serde::Deserialize)]
struct MpTokenResponse {
    #[serde(default)]
    valid: i64,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    mptokenv2: Option<String>,
}

/// POST /v2.1/clientrequestmptoken: exchange the stored session for a
/// single-use multiplayer token bound to `serverlogintoken` (an arbitrary
/// nonce minted by whoever will validate us - a game server, or the Hub for
/// publisher registration). Same call the game makes when joining a server;
/// the sessionkey itself never goes anywhere but VS's own auth server.
pub async fn request_mptoken(
    uid: &str,
    sessionkey: &str,
    serverlogintoken: &str,
) -> Result<String, String> {
    let params = [
        ("uid", uid),
        ("serverlogintoken", serverlogintoken),
        ("sessionkey", sessionkey),
    ];
    let resp = reqwest::Client::new()
        .post(format!("{AUTH_BASE}/v2.1/clientrequestmptoken"))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("mptoken request failed: {e}"))?;
    let r: MpTokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("mptoken parse failed: {e}"))?;
    if r.valid == 1 {
        r.mptokenv2.ok_or_else(|| "auth server sent no mptoken".into())
    } else {
        Err(format!(
            "VS auth server refused an mptoken: {}",
            r.reason.unwrap_or_else(|| "unknown".into())
        ))
    }
}
