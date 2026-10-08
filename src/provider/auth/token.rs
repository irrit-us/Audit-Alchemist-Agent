//! Pure helpers for the ChatGPT credential format: JWT inspection, token
//! selection, and RFC 3339 timestamps. No network or filesystem access, so this
//! module is unit-testable in isolation from the HTTP refresh path.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// Refresh a little before expiry to avoid a mid-request 401.
const REFRESH_SKEW_SECS: i64 = 120;

/// The token grant returned by the OAuth token endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
}

/// A resolved bearer token plus the optional ChatGPT account id.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub bearer: String,
    pub account_id: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("bearer", &"<redacted>")
            .field("account_id", &self.account_id)
            .finish()
    }
}

/// Return credentials when a cached access token is still valid.
pub(crate) fn usable(value: &Value) -> Option<Credentials> {
    let access = value.pointer("/tokens/access_token")?.as_str()?;
    if access.is_empty() {
        return None;
    }
    let expiry = token_expiry(value);
    match expiry {
        Some(exp) if exp <= now_secs() + REFRESH_SKEW_SECS => None,
        _ => credentials_from(value),
    }
}

fn token_expiry(value: &Value) -> Option<i64> {
    let claim = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|token| jwt_claims(token).and_then(|claims| claims.get("exp")?.as_i64()))
    };
    claim("/tokens/id_token").or_else(|| claim("/tokens/access_token"))
}

pub(crate) fn credentials_from(value: &Value) -> Option<Credentials> {
    let bearer = value
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())?
        .to_owned();
    let account_id = value
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            value
                .pointer("/tokens/id_token")
                .and_then(Value::as_str)
                .and_then(account_id_from_jwt)
        })
        .or_else(|| {
            value
                .pointer("/tokens/access_token")
                .and_then(Value::as_str)
                .and_then(account_id_from_jwt)
        });
    Some(Credentials { bearer, account_id })
}

pub(crate) fn account_id_from_jwt(token: &str) -> Option<String> {
    let claims = jwt_claims(token)?;
    claims
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Merge a refresh grant into an existing auth document, preserving fields we
/// do not manage (including the API key and any unknown keys).
pub(crate) fn update_tokens(value: &mut Value, refreshed: &TokenResponse) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    let root = value.as_object_mut().expect("object checked above");
    let tokens = root
        .entry("tokens")
        .or_insert_with(|| Value::Object(Map::new()));
    if !tokens.is_object() {
        *tokens = Value::Object(Map::new());
    }
    let tokens = tokens.as_object_mut().expect("object checked above");
    tokens.insert(
        "access_token".into(),
        Value::String(refreshed.access_token.clone()),
    );
    if let Some(refresh) = &refreshed.refresh_token {
        tokens.insert("refresh_token".into(), Value::String(refresh.clone()));
    }
    if let Some(id) = &refreshed.id_token {
        tokens.insert("id_token".into(), Value::String(id.clone()));
    }
    let account = refreshed
        .id_token
        .as_deref()
        .and_then(account_id_from_jwt)
        .or_else(|| account_id_from_jwt(&refreshed.access_token));
    if let Some(account) = account {
        tokens.insert("account_id".into(), Value::String(account));
    }
    root.insert(
        "last_refresh".into(),
        Value::String(rfc3339_utc(now_secs())),
    );
}

/// Decode a JWT payload without verifying its signature.
///
/// Verification is the server's responsibility; the client only needs `exp` and
/// the account claim, both covered by the TLS session that delivered the token.
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// Minimal unpadded base64url decoder (RFC 4648 section 5).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Some(output)
}

pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Format a Unix timestamp as RFC 3339 UTC, without pulling in a date crate.
fn rfc3339_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's days-from-civil inverse, valid for all practical dates.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: &Value) -> String {
        let header = base64url_encode(br#"{"alg":"none"}"#);
        let payload = base64url_encode(claims.to_string().as_bytes());
        format!("{header}.{payload}.sig")
    }

    fn base64url_encode(input: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(TABLE[(n >> 18 & 63) as usize] as char);
            out.push(TABLE[(n >> 12 & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[(n >> 6 & 63) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(TABLE[(n & 63) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn base64url_roundtrips_arbitrary_bytes() {
        for sample in [&b""[..], b"a", b"ab", b"abc", b"\x00\xff\x10hello"] {
            let encoded = base64url_encode(sample);
            assert_eq!(base64url_decode(&encoded).unwrap(), sample);
        }
        assert!(base64url_decode("!!!!").is_none());
    }

    #[test]
    fn jwt_claims_read_exp_and_account() {
        let token = jwt(&serde_json::json!({
            "exp": 1_700_000_000i64,
            "https://api.openai.com/auth": {"chatgpt_account_id": "acc_123"}
        }));
        let claims = jwt_claims(&token).unwrap();
        assert_eq!(claims["exp"], 1_700_000_000i64);
        assert_eq!(account_id_from_jwt(&token).as_deref(), Some("acc_123"));
        assert!(jwt_claims("not-a-jwt").is_none());
    }

    #[test]
    fn rfc3339_matches_known_instants() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_609_459_200), "2021-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn unusable_expired_token_needs_refresh() {
        let expired = serde_json::json!({
            "tokens": {"access_token": jwt(&serde_json::json!({"exp": 1i64})), "refresh_token": "r"}
        });
        assert!(usable(&expired).is_none());
        let valid = serde_json::json!({
            "tokens": {
                "access_token": jwt(&serde_json::json!({"exp": now_secs() + 3600})),
                "id_token": jwt(&serde_json::json!({
                    "exp": now_secs() + 3600,
                    "https://api.openai.com/auth": {"chatgpt_account_id": "acc_9"}
                }))
            }
        });
        let credentials = usable(&valid).unwrap();
        assert_eq!(credentials.account_id.as_deref(), Some("acc_9"));
    }

    #[test]
    fn update_tokens_preserves_unrelated_fields() {
        let mut value = serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {"refresh_token": "old", "account_id": "keep"},
            "other": {"keep": true}
        });
        let refreshed = TokenResponse {
            access_token: jwt(&serde_json::json!({"exp": now_secs() + 3600})),
            refresh_token: Some("new".into()),
            id_token: Some(jwt(&serde_json::json!({
                "https://api.openai.com/auth": {"chatgpt_account_id": "acc_new"}
            }))),
        };
        update_tokens(&mut value, &refreshed);
        assert_eq!(value["tokens"]["refresh_token"], "new");
        assert_eq!(value["tokens"]["account_id"], "acc_new");
        assert_eq!(value["other"]["keep"], true);
        assert!(value["last_refresh"].as_str().unwrap().ends_with('Z'));
    }
}
