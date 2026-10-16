//! Who is signed in (accounts-spec §2) and the folders found on a PC (§6), on invented files.
//!
//! Every file here is invented, written from the shapes Claude Code and Codex use (openai/codex
//! `AuthDotJson`, `TokenData`, `IdTokenInfo`): ids are `00000000-0000-4000-8000-00000000000N`,
//! addresses are under `example.invalid`, tokens are obviously fake
//! (`tests/fixtures/claude/README.md`).

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use lastdrop_core::model::{AccountIdentity, Identity, ProviderId};
use lastdrop_core::settings::{AccountSettings, SourceSettings};
use lastdrop_providers::discover::{self, Candidate};
use lastdrop_providers::fakes::{FakeEnv, FakeFs};
use lastdrop_providers::identity;

const ORG: &str = "00000000-0000-4000-8000-000000000001";
const ACCOUNT: &str = "00000000-0000-4000-8000-000000000002";
const EMAIL: &str = "tester@example.invalid";

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("valid timestamp")
}

fn home() -> PathBuf {
    PathBuf::from(r"C:\Users\tester")
}

fn claude_dir() -> PathBuf {
    home().join(".claude")
}

fn claude_state() -> PathBuf {
    home().join(".claude.json")
}

fn email_of(identity: &Identity) -> Option<String> {
    identity.email().map(|e| e.expose().to_owned())
}

// ---- Claude --------------------------------------------------------------------------------------

fn state_with_account(account: &str) -> String {
    format!(
        r#"{{
            "numStartups": 3,
            "projects": {{ "C:/Users/tester/src/alpha": {{ "history": [{{ "display": "invented" }}] }} }},
            "oauthAccount": {account},
            "cachedUsageUtilization": {{ "fetchedAtMs": 1791618120000 }}
        }}"#
    )
}

#[test]
fn claude_names_the_account_of_its_oauth_account() {
    let fs = FakeFs::new();
    let account = format!(
        r#"{{ "accountUuid": "{ACCOUNT}", "emailAddress": "{EMAIL}",
             "organizationUuid": "{ORG}", "organizationName": "Invented Org" }}"#
    );
    fs.add_file(claude_state(), state_with_account(&account), now());
    let identity = identity::claude(&fs, &claude_dir(), &claude_state());
    assert_eq!(
        identity.id(),
        Some(&AccountIdentity::of(&["claude", ORG, ACCOUNT]))
    );
    assert_eq!(email_of(&identity).as_deref(), Some(EMAIL));
    assert!(
        !format!("{identity:?}").contains("example.invalid"),
        "the address never prints"
    );

    // The same person in another organisation is another account.
    let other_org = format!(
        r#"{{ "accountUuid": "{ACCOUNT}", "organizationUuid": "00000000-0000-4000-8000-000000000009" }}"#
    );
    fs.add_file(claude_state(), state_with_account(&other_org), now());
    let identity = identity::claude(&fs, &claude_dir(), &claude_state());
    assert!(identity.id().is_some());
    assert_ne!(
        identity.id(),
        Some(&AccountIdentity::of(&["claude", ORG, ACCOUNT]))
    );
    assert_eq!(identity.email(), None, "no address in the file: none");
}

#[test]
fn claude_without_an_oauth_account_asks_the_credentials_file() {
    let fs = FakeFs::new();
    fs.add_file(claude_state(), r#"{ "numStartups": 1 }"#, now());
    // Nothing names a sign-in: signed out.
    assert_eq!(
        identity::claude(&fs, &claude_dir(), &claude_state()),
        Identity::SignedOut
    );
    // Only MCP sign-ins in the credentials file: still signed out.
    fs.add_file(
        claude_dir().join(".credentials.json"),
        r#"{ "mcpOAuth": { "invented|0": { "accessToken": "test-mcp-token-not-real" } } }"#,
        now(),
    );
    assert_eq!(
        identity::claude(&fs, &claude_dir(), &claude_state()),
        Identity::SignedOut
    );
    // A Claude sign-in that `.claude.json` does not name: someone, but who is unknown.
    fs.add_file(
        claude_dir().join(".credentials.json"),
        r#"{ "claudeAiOauth": { "accessToken": "test-token-not-real", "expiresAt": 0 } }"#,
        now(),
    );
    assert_eq!(
        identity::claude(&fs, &claude_dir(), &claude_state()),
        Identity::Unknown
    );
}

#[test]
fn claude_files_that_cannot_tell_are_unknown() {
    // Neither file: a folder nobody signed in to.
    let fs = FakeFs::new();
    assert_eq!(
        identity::claude(&fs, &claude_dir(), &claude_state()),
        Identity::SignedOut
    );
    // `.claude.json` cut short (Claude Code writing it).
    fs.add_file(claude_state(), r#"{ "oauthAccount": { "accountUu"#, now());
    assert_eq!(
        identity::claude(&fs, &claude_dir(), &claude_state()),
        Identity::Unknown
    );
    // An `oauthAccount` without an account id is a layout Lastdrop does not know.
    fs.add_file(
        claude_state(),
        state_with_account(&format!(r#"{{ "emailAddress": "{EMAIL}" }}"#)),
        now(),
    );
    assert_eq!(
        identity::claude(&fs, &claude_dir(), &claude_state()),
        Identity::Unknown
    );
    // Fields of other types are missing values, never a crash.
    fs.add_file(
        claude_state(),
        state_with_account(&format!(
            r#"{{ "accountUuid": "{ACCOUNT}", "emailAddress": 42, "organizationUuid": {{ "x": [1] }} }}"#
        )),
        now(),
    );
    let identity = identity::claude(&fs, &claude_dir(), &claude_state());
    assert_eq!(
        identity.id(),
        Some(&AccountIdentity::of(&["claude", "", ACCOUNT]))
    );
    assert_eq!(identity.email(), None);
}

// ---- Codex ---------------------------------------------------------------------------------------

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            let index = (n >> (18 - 6 * i)) & 63;
            out.push(char::from(ALPHABET[index as usize]));
        }
    }
    out
}

/// An unsigned JWT with these claims (Lastdrop never checks the signature: the token only names
/// the account).
fn jwt(claims: &str) -> String {
    format!(
        "{}.{}.test-signature-not-real",
        base64url(br#"{"alg":"RS256","typ":"JWT"}"#),
        base64url(claims.as_bytes())
    )
}

fn codex_home() -> PathBuf {
    home().join(".codex")
}

fn auth_json(tokens: &str, api_key: &str) -> String {
    format!(
        r#"{{ "OPENAI_API_KEY": {api_key}, "tokens": {tokens}, "last_refresh": "2026-10-09T08:00:00Z" }}"#
    )
}

fn tokens_with(id_token: &str, account_id: &str) -> String {
    format!(
        r#"{{ "id_token": "{id_token}", "access_token": "test-access-not-real",
             "refresh_token": "test-refresh-not-real", "account_id": {account_id} }}"#
    )
}

fn codex_identity(fs: &FakeFs) -> Identity {
    identity::codex(fs, &codex_home())
}

#[test]
fn codex_names_the_workspace_and_the_user_of_its_id_token() {
    let fs = FakeFs::new();
    let claims = format!(
        r#"{{ "email": "{EMAIL}", "exp": 1791655200,
             "https://api.openai.com/auth": {{ "chatgpt_plan_type": "plus",
               "chatgpt_account_id": "ws-0001", "chatgpt_user_id": "user-0001" }} }}"#
    );
    fs.add_file(
        codex_home().join("auth.json"),
        auth_json(&tokens_with(&jwt(&claims), "\"ws-0001\""), "null"),
        now(),
    );
    let identity = codex_identity(&fs);
    assert_eq!(
        identity.id(),
        Some(&AccountIdentity::of(&["codex", "ws-0001", "user-0001"]))
    );
    assert_eq!(email_of(&identity).as_deref(), Some(EMAIL));
}

#[test]
fn codex_falls_back_to_older_claims() {
    let fs = FakeFs::new();
    // `user_id` instead of `chatgpt_user_id`, the address in the profile claim, the workspace
    // only in `tokens.account_id`.
    let claims = format!(
        r#"{{ "https://api.openai.com/profile": {{ "email": "{EMAIL}" }},
             "https://api.openai.com/auth": {{ "user_id": "user-0002" }} }}"#
    );
    fs.add_file(
        codex_home().join("auth.json"),
        auth_json(&tokens_with(&jwt(&claims), "\"ws-0002\""), "null"),
        now(),
    );
    let identity = codex_identity(&fs);
    assert_eq!(
        identity.id(),
        Some(&AccountIdentity::of(&["codex", "ws-0002", "user-0002"]))
    );
    assert_eq!(email_of(&identity).as_deref(), Some(EMAIL));

    // No user id at all: the address names the user.
    let claims = format!(r#"{{ "email": "{EMAIL}" }}"#);
    fs.add_file(
        codex_home().join("auth.json"),
        auth_json(&tokens_with(&jwt(&claims), "null"), "null"),
        now(),
    );
    assert_eq!(
        codex_identity(&fs).id(),
        Some(&AccountIdentity::of(&["codex", "", EMAIL]))
    );

    // Neither: Lastdrop cannot tell who it is.
    fs.add_file(
        codex_home().join("auth.json"),
        auth_json(&tokens_with(&jwt(r#"{ "exp": 1 }"#), "null"), "null"),
        now(),
    );
    assert_eq!(codex_identity(&fs), Identity::Unknown);
}

#[test]
fn codex_without_tokens_is_signed_out_or_an_api_key() {
    let fs = FakeFs::new();
    fs.add_file(
        codex_home().join("auth.json"),
        r#"{ "OPENAI_API_KEY": null }"#,
        now(),
    );
    assert_eq!(codex_identity(&fs), Identity::SignedOut);
    fs.add_file(
        codex_home().join("auth.json"),
        r#"{ "OPENAI_API_KEY": "test-api-key-not-real" }"#,
        now(),
    );
    assert_eq!(codex_identity(&fs), Identity::Unknown);
}

#[test]
fn codex_files_that_cannot_tell_are_unknown() {
    let fs = FakeFs::new();
    // No `auth.json`: the sign-in may be in the Windows credential store.
    assert_eq!(codex_identity(&fs), Identity::Unknown);
    // Not JSON.
    fs.add_file(codex_home().join("auth.json"), "not json", now());
    assert_eq!(codex_identity(&fs), Identity::Unknown);
    // An ID token that is not a JWT, or whose payload is not base64url JSON.
    for token in [
        "not-a-jwt",
        "a.b",
        "a.b.c.d",
        "eyJhbGciOiJub25lIn0.***.sig",
        "eyJhbGciOiJub25lIn0.bm90IGpzb24.sig",
    ] {
        fs.add_file(
            codex_home().join("auth.json"),
            auth_json(&tokens_with(token, "null"), "null"),
            now(),
        );
        assert_eq!(codex_identity(&fs), Identity::Unknown, "{token}");
    }
}

// ---- Through the source ----------------------------------------------------------------------------

#[test]
fn read_dispatches_on_the_source() {
    let fs = FakeFs::new();
    let env = FakeEnv::new(home());
    let account = format!(r#"{{ "accountUuid": "{ACCOUNT}", "organizationUuid": "{ORG}" }}"#);
    fs.add_file(claude_state(), state_with_account(&account), now());
    let claude = lastdrop_providers::AccountSource::resolve(
        ProviderId::Claude,
        &AccountSettings::primary(),
        &env,
    )
    .expect("resolve");
    assert!(identity::read(&fs, &claude).id().is_some());
    let meshy = lastdrop_providers::AccountSource::resolve(
        ProviderId::Meshy,
        &AccountSettings::primary(),
        &env,
    )
    .expect("resolve");
    assert_eq!(identity::read(&fs, &meshy), Identity::Unknown);
}

// ---- Discovery (accounts-spec §6) ------------------------------------------------------------------

fn file(fs: &FakeFs, path: &Path) {
    fs.add_file(path, "{}", now());
}

#[test]
fn discovery_lists_the_default_folder_then_the_extra_ones_with_data() {
    let fs = FakeFs::new();
    let env = FakeEnv::new(home());
    file(&fs, &home().join(r".claude\.credentials.json"));
    file(&fs, &home().join(".claude.json"));
    file(&fs, &home().join(r".claude-work\settings.json"));
    file(
        &fs,
        &home().join(r".claude_old\projects\alpha\session.jsonl"),
    );
    // Not Claude data, a file rather than a folder, and a prefix alone.
    file(&fs, &home().join(r".claude-empty\notes.txt"));
    file(&fs, &home().join(".claude-backup.json"));
    file(&fs, &home().join(r".claude-\settings.json"));
    file(&fs, &home().join(r".codex-work\auth.json"));
    file(
        &fs,
        &home().join(r".codex-play\sessions\2026\10\rollout.jsonl"),
    );
    let claude = discover::candidates(ProviderId::Claude, &env, &fs);
    assert_eq!(
        claude,
        vec![
            Candidate {
                folder: home().join(".claude"),
                source: SourceSettings::Default,
            },
            Candidate {
                folder: home().join(".claude-work"),
                source: SourceSettings::ClaudeConfigDir {
                    path: home().join(".claude-work"),
                },
            },
            Candidate {
                folder: home().join(".claude_old"),
                source: SourceSettings::ClaudeConfigDir {
                    path: home().join(".claude_old"),
                },
            },
        ]
    );
    // No `~\.codex` data: only the extra folders, sorted.
    let codex = discover::candidates(ProviderId::Codex, &env, &fs);
    let folders: Vec<&PathBuf> = codex.iter().map(|c| &c.folder).collect();
    assert_eq!(
        folders,
        [&home().join(".codex-play"), &home().join(".codex-work")]
    );
    assert!(discover::candidates(ProviderId::Meshy, &env, &fs).is_empty());
}

#[test]
fn discovery_leaves_out_folders_that_are_accounts_already() {
    let fs = FakeFs::new();
    file(&fs, &home().join(r".claude-work\settings.json"));
    // CLAUDE_CONFIG_DIR points at ~\.claude-work (written with a trailing separator): that is the
    // default folder now, listed once.
    let env = FakeEnv::new(home()).with_var("CLAUDE_CONFIG_DIR", home().join(".claude-work\\"));
    let found = discover::candidates(ProviderId::Claude, &env, &fs);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].source, SourceSettings::Default);

    let env = FakeEnv::new(home());
    file(&fs, &home().join(r".claude-play\settings.json"));
    let accounts = vec![
        AccountSettings::primary(),
        AccountSettings::for_folder(
            SourceSettings::ClaudeConfigDir {
                path: PathBuf::from(r"c:\users\TESTER\.claude-work"),
            },
            None,
            &[],
        ),
    ];
    let found = discover::candidates(ProviderId::Claude, &env, &fs);
    let unused = discover::unused(ProviderId::Claude, found, &accounts, &env);
    let folders: Vec<&PathBuf> = unused.iter().map(|c| &c.folder).collect();
    assert_eq!(folders, [&home().join(".claude-play")]);
}

#[test]
fn folders_compare_as_windows_does() {
    assert!(discover::same_folder(
        Path::new(r"C:\Users\Tester\.claude"),
        Path::new("c:/users/tester/.claude/")
    ));
    assert!(!discover::same_folder(
        Path::new(r"C:\Users\tester\.claude"),
        Path::new(r"C:\Users\tester\.claude-work")
    ));
}
