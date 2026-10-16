//! Who is signed in to a source (accounts-spec §2), read from the source's own files, read-only,
//! with the same rules as a fetch (ARCHITECTURE "Rules for every provider"): nothing is written,
//! refreshed or copied; the ids are held only as long as it takes to hash them and the address
//! as a [`SecretString`], and every buffer that held them is overwritten when it is dropped (best
//! effort, as `SecretString` does); nothing here is logged.
//!
//! - **Claude**: `.claude.json`'s `oauthAccount` (streamed, everything else skipped), and whether
//!   `.credentials.json` holds a `claudeAiOauth` sign-in.
//! - **Codex**: `auth.json`'s `tokens.id_token` payload (a JWT, read and not verified: it only
//!   names the account here) and `tokens.account_id`.
//!
//! Used by the refresh workers (`Provider::identity`) and by the settings window, which has no
//! [`crate::FetchContext`]: so everything takes an [`Fs`].

use std::fmt;
use std::path::Path;

use lastdrop_core::model::{AccountIdentity, Identity};
use lastdrop_core::secret::SecretString;
use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::AccountSource;
use crate::claude::{cache, oauth};
use crate::io::Fs;

/// Codex's sign-in file in `CODEX_HOME`.
pub const CODEX_AUTH_FILE: &str = "auth.json";

/// Largest `auth.json` read (it holds three tokens; a few kilobytes).
const MAX_AUTH_BYTES: u64 = 64 << 10;

/// Who is signed in to `source`. Copilot and Meshy have one account each in M2 and are
/// [`Identity::Unknown`].
pub fn read(fs: &dyn Fs, source: &AccountSource) -> Identity {
    match source {
        AccountSource::Claude {
            config_dir,
            state_file,
            ..
        } => claude(fs, config_dir, state_file),
        AccountSource::Codex { codex_home, .. } => codex(fs, codex_home),
        AccountSource::Copilot { .. } | AccountSource::Meshy { .. } => Identity::Unknown,
    }
}

// ---- Claude ----------------------------------------------------------------------------------

/// `.claude.json`: only `oauthAccount` is kept while streaming.
#[derive(Deserialize)]
struct ClaudeState {
    #[serde(rename = "oauthAccount", default)]
    account: Option<OAuthAccount>,
}

#[derive(Deserialize)]
struct OAuthAccount {
    #[serde(rename = "accountUuid", default)]
    account: Text,
    #[serde(rename = "organizationUuid", default)]
    organisation: Text,
    #[serde(rename = "emailAddress", default)]
    email: Text,
}

/// `.credentials.json`: only whether a Claude sign-in is there (nothing of it is kept).
#[derive(Deserialize)]
struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth", default)]
    oauth: Option<IgnoredAny>,
}

/// What reading one file gave.
enum Read<T> {
    /// Parsed.
    Parsed(T),
    /// Not there.
    Missing,
    /// There, but unreadable or not JSON of the expected shape.
    Unreadable,
}

/// Claude Code (accounts-spec §2 "Claude"): `oauthAccount` with an account id names the account
/// (hash of `claude`, the organisation and the account); without it, a `claudeAiOauth` sign-in in
/// `.credentials.json` is someone Lastdrop cannot name (`Unknown`), and neither is `SignedOut`. A
/// file that is there but cannot be read makes the answer `Unknown` unless the other one names
/// the account.
pub fn claude(fs: &dyn Fs, config_dir: &Path, state_file: &Path) -> Identity {
    let state = match fs.open(state_file, cache::MAX_BYTES) {
        Ok(reader) => match serde_json::from_reader::<_, ClaudeState>(reader) {
            Ok(state) => Read::Parsed(state.account),
            Err(_) => Read::Unreadable,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Unreadable,
    };
    if let Read::Parsed(Some(account)) = &state
        && let Some(id) = non_empty(&account.account.0)
    {
        let organisation = non_empty(&account.organisation.0).unwrap_or("");
        let identity = AccountIdentity::of(&["claude", organisation, id]);
        let email = non_empty(&account.email.0).map(|e| SecretString::new(e.to_owned()));
        return Identity::SignedIn {
            id: identity,
            email,
        };
    }
    let credentials = match fs.read(
        &oauth::credentials_path(config_dir),
        oauth::MAX_CREDENTIALS_BYTES,
    ) {
        Ok(mut bytes) => {
            let parsed = serde_json::from_slice::<ClaudeCredentials>(&bytes);
            wipe(&mut bytes);
            match parsed {
                Ok(file) => Read::Parsed(file.oauth.is_some()),
                Err(_) => Read::Unreadable,
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Unreadable,
    };
    match (state, credentials) {
        (_, Read::Parsed(true)) | (Read::Unreadable, _) | (_, Read::Unreadable) => {
            Identity::Unknown
        }
        // An `oauthAccount` without an account id is a layout Lastdrop does not know.
        (Read::Parsed(Some(_)), _) => Identity::Unknown,
        (Read::Parsed(None) | Read::Missing, Read::Parsed(false) | Read::Missing) => {
            Identity::SignedOut
        }
    }
}

// ---- Codex -----------------------------------------------------------------------------------

/// `auth.json` (openai/codex `AuthDotJson`): the API key's presence and the tokens' account
/// fields; the access and refresh tokens are skipped while parsing.
#[derive(Deserialize)]
struct CodexAuth {
    #[serde(rename = "OPENAI_API_KEY", default)]
    api_key: Option<IgnoredAny>,
    #[serde(default)]
    tokens: Option<CodexTokens>,
}

#[derive(Deserialize)]
struct CodexTokens {
    /// The ID token, a JWT (openai/codex `TokenData::id_token`, stored as the raw JWT).
    #[serde(default)]
    id_token: Text,
    /// The workspace (`TokenData::account_id`).
    #[serde(default)]
    account_id: Text,
}

/// The ID token's claims Lastdrop reads (openai/codex `IdTokenInfo`).
#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    email: Text,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize)]
struct ProfileClaims {
    #[serde(default)]
    email: Text,
}

#[derive(Deserialize)]
struct AuthClaims {
    #[serde(default)]
    chatgpt_account_id: Text,
    #[serde(default)]
    chatgpt_user_id: Text,
    #[serde(default)]
    user_id: Text,
}

/// Codex (accounts-spec §2 "Codex"): the ID token names the workspace (`chatgpt_account_id`, else
/// `tokens.account_id`) and the user (`chatgpt_user_id`, else `user_id`, else the e-mail); the
/// identity is the hash of `codex`, the workspace and the user. `auth.json` with neither tokens
/// nor an API key is `SignedOut`; an API-key sign-in, tokens Lastdrop cannot read, a missing
/// `auth.json` (Codex may keep its sign-in in the Windows credential store, which Lastdrop never
/// reads) or an unreadable one are `Unknown`.
pub fn codex(fs: &dyn Fs, codex_home: &Path) -> Identity {
    let Ok(mut bytes) = fs.read(&codex_home.join(CODEX_AUTH_FILE), MAX_AUTH_BYTES) else {
        return Identity::Unknown;
    };
    let parsed = serde_json::from_slice::<CodexAuth>(&bytes);
    wipe(&mut bytes);
    let Ok(auth) = parsed else {
        return Identity::Unknown;
    };
    let Some(mut tokens) = auth.tokens else {
        return if auth.api_key.is_some() {
            Identity::Unknown
        } else {
            Identity::SignedOut
        };
    };
    let token = tokens.id_token.0.take().map(SecretString::new);
    let claims = token.as_ref().and_then(|t| jwt_claims(t.expose()));
    let Some(claims) = claims else {
        return Identity::Unknown;
    };
    let email = non_empty(&claims.email.0)
        .or_else(|| claims.profile.as_ref().and_then(|p| non_empty(&p.email.0)))
        .map(|e| SecretString::new(e.to_owned()));
    let auth_claims = claims.auth.as_ref();
    let workspace = auth_claims
        .and_then(|a| non_empty(&a.chatgpt_account_id.0))
        .or_else(|| non_empty(&tokens.account_id.0))
        .unwrap_or("");
    let user = auth_claims
        .and_then(|a| non_empty(&a.chatgpt_user_id.0).or_else(|| non_empty(&a.user_id.0)))
        .or_else(|| email.as_ref().map(SecretString::expose));
    match user {
        Some(user) => Identity::SignedIn {
            id: AccountIdentity::of(&["codex", workspace, user]),
            email,
        },
        None => Identity::Unknown,
    }
}

/// The claims of a JWT (`header.payload.signature`, base64url): read, not verified. `None` when it
/// is not a JWT or its payload is not a JSON object of the expected shape.
fn jwt_claims(token: &str) -> Option<Claims> {
    let mut parts = token.trim().split('.');
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let mut json = base64url_decode(payload)?;
    let claims = serde_json::from_slice::<Claims>(&json).ok();
    wipe(&mut json);
    claims
}

/// Decodes base64url (RFC 4648 §5), with or without `=` padding. `None` on any other character or
/// an impossible length.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => {
                wipe(&mut out);
                return None;
            }
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

// ---- Helpers ---------------------------------------------------------------------------------

/// A string field that may be missing, `null` or of another type (then `None`), read without
/// buffering the value (unlike `claude::json::Lenient`), so nothing of it lingers on the heap but
/// the string itself, which is overwritten when it drops (like `SecretString`: the fields hold
/// ids and addresses).
#[derive(Default)]
struct Text(Option<String>);

impl Drop for Text {
    fn drop(&mut self) {
        if let Some(text) = &mut self.0 {
            let len = text.len();
            text.clear();
            text.extend(std::iter::repeat_n('\0', len));
            std::hint::black_box(text.as_bytes());
        }
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Text, D::Error> {
        deserializer.deserialize_any(TextVisitor)
    }
}

struct TextVisitor;

impl<'de> Visitor<'de> for TextVisitor {
    type Value = Text;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
        Ok(Text(Some(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Text, E> {
        Ok(Text(Some(v)))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_none<E: de::Error>(self) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Text, D::Error> {
        deserializer.deserialize_any(TextVisitor)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Text, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Text(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Text, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Text(None))
    }
}

/// The text when it is there and not blank.
fn non_empty(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// Overwrites a buffer that held a credential file (best effort, like `SecretString`'s drop).
fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(&*bytes);
}

#[cfg(test)]
mod tests {
    use super::base64url_decode;

    #[test]
    fn base64url_round_trips_the_rfc_4648_vectors() {
        for (encoded, plain) in [
            ("", ""),
            ("Zg", "f"),
            ("Zm8", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg", "foob"),
            ("Zm9vYmE", "fooba"),
            ("Zm9vYmFy", "foobar"),
            ("Zm9vYg==", "foob"),
        ] {
            assert_eq!(
                base64url_decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "{encoded}"
            );
        }
        // The URL alphabet: `-` and `_` instead of `+` and `/`.
        assert_eq!(base64url_decode("-_8").as_deref(), Some(&[0xfb, 0xff][..]));
        assert_eq!(base64url_decode("+/8"), None);
        assert_eq!(base64url_decode("Z"), None, "an impossible length");
        assert_eq!(base64url_decode("Zm9v!"), None);
    }
}
