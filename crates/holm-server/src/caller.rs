use crate::error::ApiError;
use axum::http::{Method, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use holm_api::ErrorCode;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONSOLE_KEY: &str = "HOLM_CONSOLE_PUBLIC_KEY";
pub const CONSOLE_KEY_FILE: &str = "HOLM_CONSOLE_PUBLIC_KEY_FILE";

const ISSUER: &str = "console";
const AUDIENCE: &str = "holmd";
const AUDIENCE_BEFORE: &str = "computerd";
const LEEWAY_SECS: u64 = 30;
const SPKI_ED25519: usize = 44;
const RAW_ED25519: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Member,
    Admin,
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub owner: Option<String>,
    pub role: Role,
}

impl Caller {
    pub fn operator() -> Self {
        Self {
            owner: None,
            role: Role::Operator,
        }
    }

    pub fn of(owner: &str, role: Role) -> Self {
        Self {
            owner: Some(owner.to_string()),
            role,
        }
    }

    pub fn sees(&self, owner: Option<&str>) -> bool {
        match &self.owner {
            None => self.role == Role::Operator,
            Some(mine) => owner == Some(mine.as_str()),
        }
    }

    pub fn sees_runtime(&self, owner: Option<&str>) -> bool {
        owner.is_none() || self.sees(owner)
    }

    pub fn may(&self, role: Role) -> bool {
        self.role >= role
    }

    pub fn owns_what_it_makes(&self) -> Option<String> {
        self.owner.clone()
    }
}

pub struct ConsoleKey(Vec<u8>);

impl ConsoleKey {
    pub fn from_env() -> Result<Option<Self>, String> {
        let given = match std::env::var(CONSOLE_KEY)
            .ok()
            .filter(|key| !key.is_empty())
        {
            Some(key) => key,
            None => match std::env::var(CONSOLE_KEY_FILE)
                .ok()
                .filter(|at| !at.is_empty())
            {
                Some(at) => {
                    std::fs::read_to_string(&at).map_err(|error| format!("{at}: {error}"))?
                }
                None => return Ok(None),
            },
        };
        Self::parse(&given).map(Some)
    }

    pub fn parse(given: &str) -> Result<Self, String> {
        let body: String = given
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string();
        let bytes = STANDARD
            .decode(&body)
            .map_err(|_| format!("{CONSOLE_KEY} is an Ed25519 public key in base64 or PEM"))?;

        match bytes.len() {
            RAW_ED25519 => Ok(Self(bytes)),
            SPKI_ED25519 => Ok(Self(bytes.split_at(SPKI_ED25519 - RAW_ED25519).1.to_vec())),
            other => Err(format!(
                "{CONSOLE_KEY} holds {other} bytes; an Ed25519 public key is {RAW_ED25519}, or {SPKI_ED25519} as SPKI"
            )),
        }
    }

    pub fn raw(bytes: Vec<u8>) -> Result<Self, String> {
        match bytes.len() {
            RAW_ED25519 => Ok(Self(bytes)),
            other => Err(format!(
                "an Ed25519 public key is {RAW_ED25519} bytes, not {other}"
            )),
        }
    }

    pub fn verifies(&self, token: &Token) -> bool {
        UnparsedPublicKey::new(&ED25519, &self.0)
            .verify(token.signed.as_bytes(), &token.signature)
            .is_ok()
    }

    pub fn verify(&self, token: &str, now_secs: u64) -> Option<Caller> {
        let token = Token::parse(token)?;
        match self.verifies(&token) {
            true => token.caller(Kind::Call, now_secs),
            false => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Call,
    Key,
}

pub struct Token {
    pub kid: Option<String>,
    signed: String,
    signature: Vec<u8>,
    claims: Claims,
}

impl Token {
    pub fn parse(token: &str) -> Option<Self> {
        let (signed, signature) = token.rsplit_once('.')?;
        let (header, payload) = signed.split_once('.')?;
        if payload.contains('.') {
            return None;
        }

        let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
        if header.alg != "EdDSA" {
            return None;
        }

        Some(Self {
            kid: header.kid,
            signed: signed.to_string(),
            signature: URL_SAFE_NO_PAD.decode(signature).ok()?,
            claims: serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?,
        })
    }

    pub fn key_id(&self) -> Option<&str> {
        self.claims.sub.as_deref()
    }

    pub fn workspace(&self) -> &str {
        &self.claims.ws
    }

    pub fn caller(&self, kind: Kind, now_secs: u64) -> Option<Caller> {
        let claims = &self.claims;
        let unexpired = match (kind, claims.exp) {
            (_, Some(exp)) => exp.saturating_add(LEEWAY_SECS) > now_secs,
            (Kind::Key, None) => true,
            (Kind::Call, None) => false,
        };
        let started = claims.nbf.unwrap_or(0) <= now_secs.saturating_add(LEEWAY_SECS);
        let named = kind == Kind::Call || claims.sub.as_deref().is_some_and(|sub| !sub.is_empty());

        match (
            claims.iss == ISSUER
                && (claims.aud == AUDIENCE || claims.aud == AUDIENCE_BEFORE)
                && claims.kind == kind,
            unexpired && started && named,
            claims.role,
        ) {
            (true, true, Role::Viewer | Role::Member | Role::Admin) if !claims.ws.is_empty() => {
                Some(Caller::of(&claims.ws, claims.role))
            }
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: String,
    ws: String,
    role: Role,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(default)]
    nbf: Option<u64>,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    kind: Kind,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Box(String),
    Runtime(String),
    Nothing,
}

pub fn target_of(path: &str) -> Target {
    let mut segments = path.trim_start_matches('/').split('/');
    match (segments.next(), segments.next(), segments.next()) {
        (Some("v1"), Some("boxes"), Some(id)) if !id.is_empty() => Target::Box(id.to_string()),
        (Some("v1"), Some("runtimes"), Some(name)) if !name.is_empty() => {
            Target::Runtime(name.to_string())
        }
        _ => Target::Nothing,
    }
}

pub fn needs(method: &Method, path: &str) -> Role {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let reads = matches!(*method, Method::GET | Method::HEAD);

    match segments.as_slice() {
        ["mcp"] => Role::Viewer,
        ["v1", "jobs", _] => Role::Operator,
        _ if reads => Role::Viewer,
        ["v1", "boxes", _, "screens", _, "viewer", "ticket"] => Role::Viewer,
        ["v1", "runtimes"] => Role::Admin,
        ["v1", "runtimes", _] => Role::Admin,
        _ => Role::Member,
    }
}

pub fn refused(caller: &Caller, needed: Role) -> Option<ApiError> {
    (!caller.may(needed)).then(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Denied,
            format!(
                "this needs the {needed:?} role in the workspace, and the caller has {:?}",
                caller.role
            )
            .to_lowercase(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn signed(pair: &Ed25519KeyPair, claims: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"EdDSA","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signature = pair.sign(format!("{header}.{payload}").as_bytes());
        format!(
            "{header}.{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        )
    }

    fn pair() -> Ed25519KeyPair {
        let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("a key");
        Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("a pair")
    }

    fn claims(role: &str, exp: u64) -> serde_json::Value {
        serde_json::json!({ "iss": "console", "aud": "holmd", "sub": "usr_1", "ws": "ws_a", "role": role, "exp": exp })
    }

    #[test]
    fn test_a_console_token_names_its_workspace_and_role() {
        let pair = pair();
        let key = ConsoleKey::parse(&STANDARD.encode(pair.public_key().as_ref())).expect("a key");

        assert_eq!(
            key.verify(&signed(&pair, claims("member", 2_000)), 1_000),
            Some(Caller::of("ws_a", Role::Member))
        );
    }

    #[test]
    fn test_a_token_that_is_old_forged_or_for_an_operator_is_refused() {
        let pair = pair();
        let key = ConsoleKey::parse(&STANDARD.encode(pair.public_key().as_ref())).expect("a key");

        assert_eq!(
            key.verify(&signed(&pair, claims("member", 1_000)), 2_000),
            None,
            "expired"
        );
        assert_eq!(
            key.verify(&signed(&pair, claims("operator", 2_000)), 1_000),
            None,
            "a console cannot mint an operator"
        );
        assert_eq!(
            key.verify(&signed(&self::pair(), claims("admin", 2_000)), 1_000),
            None,
            "another key signed it"
        );

        let mut wrong = claims("admin", 2_000);
        wrong["aud"] = "somewhere-else".into();
        assert_eq!(
            key.verify(&signed(&pair, wrong), 1_000),
            None,
            "meant for another service"
        );
        assert_eq!(key.verify("not.a.token", 1_000), None);
    }

    #[test]
    fn test_a_call_token_and_an_api_key_cannot_stand_in_for_each_other() {
        let pair = pair();
        let key = ConsoleKey::parse(&STANDARD.encode(pair.public_key().as_ref())).expect("a key");
        let mut api_key = claims("member", 2_000);
        api_key["kind"] = "key".into();
        api_key["sub"] = "key_1".into();
        api_key.as_object_mut().expect("claims").remove("exp");

        let token = Token::parse(&signed(&pair, api_key)).expect("a token");
        assert!(key.verifies(&token));
        assert_eq!(
            token.caller(Kind::Key, 1_000),
            Some(Caller::of("ws_a", Role::Member)),
            "an API key may never expire"
        );
        assert_eq!(
            token.caller(Kind::Call, 1_000),
            None,
            "but it is not a call token"
        );

        let call = Token::parse(&signed(&pair, claims("member", 2_000))).expect("a token");
        assert_eq!(
            call.caller(Kind::Key, 1_000),
            None,
            "and a call token is not an API key"
        );

        let mut endless = claims("member", 0);
        endless.as_object_mut().expect("claims").remove("exp");
        let endless = Token::parse(&signed(&pair, endless)).expect("a token");
        assert_eq!(
            endless.caller(Kind::Call, 1_000),
            None,
            "a call token must expire"
        );
    }

    #[test]
    fn test_a_pem_public_key_reads_the_same_as_a_raw_one() {
        let pair = pair();
        let mut spki = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        spki.extend_from_slice(pair.public_key().as_ref());
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(&spki)
        );

        let key = ConsoleKey::parse(&pem).expect("a PEM key");
        assert!(
            key.verify(&signed(&pair, claims("viewer", 2_000)), 1_000)
                .is_some()
        );
    }

    #[test]
    fn test_a_workspace_sees_its_own_and_shared_things_and_an_operator_sees_all() {
        let member = Caller::of("ws_a", Role::Member);

        assert!(member.sees(Some("ws_a")));
        assert!(!member.sees(Some("ws_b")));
        assert!(!member.sees(None), "a box with no owner is the operator's");
        assert!(
            member.sees_runtime(None),
            "a runtime with no owner is shared"
        );
        assert!(Caller::operator().sees(Some("ws_b")) && Caller::operator().sees(None));
    }

    #[test]
    fn test_reading_needs_a_viewer_and_changing_runtimes_needs_an_admin() {
        assert_eq!(needs(&Method::GET, "/v1/boxes/box_1/trace"), Role::Viewer);
        assert_eq!(
            needs(&Method::POST, "/v1/boxes/box_1/screens/0/viewer/ticket"),
            Role::Viewer
        );
        assert_eq!(
            needs(&Method::POST, "/v1/boxes/box_1/screens/0/actions"),
            Role::Member
        );
        assert_eq!(needs(&Method::POST, "/v1/boxes"), Role::Member);
        assert_eq!(needs(&Method::POST, "/v1/runtimes"), Role::Admin);
        assert_eq!(needs(&Method::PATCH, "/v1/runtimes/e2b"), Role::Admin);
        assert_eq!(needs(&Method::POST, "/v1/runtimes/e2b/image"), Role::Member);
        assert_eq!(
            needs(&Method::DELETE, "/v1/runtimes/e2b/images/abc"),
            Role::Member
        );
        assert_eq!(
            needs(&Method::POST, "/mcp"),
            Role::Viewer,
            "each REST call it makes is checked"
        );

        assert_eq!(
            target_of("/v1/boxes/box_1/screens/0"),
            Target::Box("box_1".into())
        );
        assert_eq!(
            target_of("/v1/runtimes/e2b/images"),
            Target::Runtime("e2b".into())
        );
        assert_eq!(target_of("/v1/boxes"), Target::Nothing);
    }
}
