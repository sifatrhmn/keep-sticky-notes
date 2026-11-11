//! Google sign-in for the Keep sync protocol.
//! The password is only ever typed into Google's own page. The application receives a
//! short-lived `oauth_token` cookie and exchanges it for a long-lived master token.

use crate::keep::KeepError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const AUTH_URL: &str = "https://android.clients.google.com/auth";
const CLIENT_SIG: &str = "38918a453d07199354f8b19af05ec6562ced5788";
const KEEP_APP: &str = "com.google.android.keep";
const KEEP_SCOPES: &str = "oauth2:https://www.googleapis.com/auth/memento https://www.googleapis.com/auth/reminders";
const PLAY_SERVICES: &str = "240913000";
pub const SIGN_IN_URL: &str = "https://accounts.google.com/EmbeddedSetup";
pub const COOKIE_NAME: &str = "oauth_token";

#[derive(Clone)]
pub struct Account {
    pub email: String,
    pub master_token: String,
    pub android_id: String,
}

pub fn http_agent() -> Result<ureq::Agent, KeepError> {
    let tls = native_tls::TlsConnector::new().map_err(|e| KeepError::Network(e.to_string()))?;
    Ok(ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).build())
}

pub fn new_android_id() -> String {
    format!("{:016x}", fastrand::u64(..))
}

pub fn parse_kv(body: &str) -> HashMap<String, String> {
    body.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn post_form(agent: &ureq::Agent, form: &[(&str, &str)]) -> Result<HashMap<String, String>, KeepError> {
    let result = agent
        .post(AUTH_URL)
        .set("User-Agent", "GoogleAuth/1.4")
        .set("Accept-Encoding", "identity")
        .timeout(Duration::from_secs(30))
        .send_form(form);
    let body = match result {
        Ok(r) => r.into_string(),
        // Google reports failures such as BadAuthentication in the body of a 4xx response.
        Err(ureq::Error::Status(_, r)) => r.into_string(),
        Err(e) => return Err(KeepError::Network(e.to_string())),
    }
    .map_err(|e| KeepError::Network(e.to_string()))?;
    Ok(parse_kv(&body))
}

fn describe(map: &HashMap<String, String>) -> String {
    map.get("Error").cloned().unwrap_or_else(|| "no token in Google's response".to_string())
}

/// Exchanges the cookie from Google's sign-in page for a master token.
pub fn exchange_cookie(email: &str, oauth_token: &str, android_id: &str) -> Result<String, KeepError> {
    let agent = http_agent()?;
    let map = post_form(
        &agent,
        &[
            ("accountType", "HOSTED_OR_GOOGLE"),
            ("Email", email),
            ("has_permission", "1"),
            ("add_account", "1"),
            ("ACCESS_TOKEN", "1"),
            ("Token", oauth_token),
            ("service", "ac2dm"),
            ("source", "android"),
            ("androidId", android_id),
            ("device_country", "us"),
            ("operatorCountry", "us"),
            ("lang", "en"),
            ("sdk_version", "17"),
            ("google_play_services_version", PLAY_SERVICES),
            ("client_sig", CLIENT_SIG),
            ("callerSig", CLIENT_SIG),
            ("droidguard_results", "dummy123"),
        ],
    )?;
    map.get("Token").cloned().ok_or_else(|| KeepError::Auth(describe(&map)))
}

/// Obtains a short-lived access token for the Keep API.
pub fn fetch_access_token(agent: &ureq::Agent, account: &Account) -> Result<(String, Duration), KeepError> {
    let map = post_form(
        agent,
        &[
            ("accountType", "HOSTED_OR_GOOGLE"),
            ("Email", account.email.as_str()),
            ("has_permission", "1"),
            ("EncryptedPasswd", account.master_token.as_str()),
            ("service", KEEP_SCOPES),
            ("source", "android"),
            ("androidId", account.android_id.as_str()),
            ("app", KEEP_APP),
            ("client_sig", CLIENT_SIG),
            ("device_country", "us"),
            ("operatorCountry", "us"),
            ("lang", "en"),
            ("sdk_version", "17"),
            ("google_play_services_version", PLAY_SERVICES),
        ],
    )?;
    let token = map.get("Auth").cloned().ok_or_else(|| KeepError::Auth(describe(&map)))?;
    let lifetime = map.get("ExpiresInDurationSec").and_then(|s| s.parse().ok()).unwrap_or(1800);
    Ok((token, Duration::from_secs(lifetime)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_key_value_lines() {
        let m = super::parse_kv("Auth=abc=def\nExpiry=1\n\nnoequals");
        assert_eq!(m["Auth"], "abc=def");
        assert_eq!(m.len(), 2);
    }
}
