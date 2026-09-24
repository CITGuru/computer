use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer::Secret;
use computer_storage::Sealed;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use std::path::Path;

pub const KEY: &str = "COMPUTER_SERVER_SECRET_KEY";
pub const KEY_FILE: &str = "COMPUTER_SERVER_SECRET_FILE";

const NONCE: usize = 12;
const LENGTH: usize = 32;

#[derive(Default)]
pub struct Keeper {
    key: Option<LessSafeKey>,
}

impl Keeper {
    pub fn from_env() -> Result<Self, String> {
        if let Ok(given) = std::env::var(KEY)
            && !given.is_empty()
        {
            return Self::of(&read(&given)?);
        }

        match std::env::var(KEY_FILE).ok().filter(|at| !at.is_empty()) {
            Some(at) => Self::of(&kept(Path::new(&at))?),
            None => Ok(Self::default()),
        }
    }

    pub fn of(key: &[u8]) -> Result<Self, String> {
        if key.len() != LENGTH {
            return Err(format!(
                "a server key is {LENGTH} bytes and this one is {}",
                key.len()
            ));
        }

        let key = UnboundKey::new(&CHACHA20_POLY1305, key)
            .map_err(|_| "this key cannot seal anything".to_string())?;

        Ok(Self {
            key: Some(LessSafeKey::new(key)),
        })
    }

    pub fn holds_a_key(&self) -> bool {
        self.key.is_some()
    }

    pub fn seal(&self, whose: &Whose, secret: &Secret) -> Result<Sealed, String> {
        let key = self.key.as_ref().ok_or_else(|| {
            format!(
                "this server keeps no secrets: set {KEY} or {KEY_FILE} before storing one, \
                 or name the runtime in the configuration file instead"
            )
        })?;

        let mut nonce = [0u8; NONCE];
        getrandom::fill(&mut nonce)
            .map_err(|error| format!("no randomness for a nonce: {error}"))?;

        let mut body = secret.expose().as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(whose.bound()),
            &mut body,
        )
        .map_err(|_| "this secret would not seal".to_string())?;

        let mut carried = nonce.to_vec();
        carried.extend_from_slice(&body);

        Ok(Sealed::of(BASE64.encode(carried)))
    }

    pub fn open(&self, whose: &Whose, sealed: &Sealed) -> Result<Secret, String> {
        let key = self.key.as_ref().ok_or_else(|| {
            format!(
                "{} was sealed with a key this server does not have",
                whose.of
            )
        })?;

        let carried = BASE64
            .decode(sealed.as_str())
            .map_err(|_| format!("{} is not a sealed secret", whose.of))?;

        if carried.len() <= NONCE {
            return Err(format!("{} is not a sealed secret", whose.of));
        }

        let (nonce, body) = carried.split_at(NONCE);
        let mut nonce_bytes = [0u8; NONCE];
        nonce_bytes.copy_from_slice(nonce);
        let mut body = body.to_vec();

        let opened = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(whose.bound()),
                &mut body,
            )
            .map_err(|_| {
                format!(
                    "{} does not open with this server's key: it was sealed with another, \
                     or it belongs to another runtime",
                    whose.of
                )
            })?;

        let text = String::from_utf8(opened.to_vec())
            .map_err(|_| format!("{} opened into something that is not a secret", whose.of))?;

        Secret::new(text).map_err(|error| error.to_string())
    }
}

pub struct Whose {
    pub of: String,
    bound: Vec<u8>,
}

impl Whose {
    pub fn new(runtime: &str, provider: &str, field: &str) -> Self {
        Self {
            of: format!("{runtime}.{field}"),
            bound: format!("{runtime}\0{provider}\0{field}").into_bytes(),
        }
    }

    fn bound(&self) -> &[u8] {
        &self.bound
    }
}

fn read(given: &str) -> Result<Vec<u8>, String> {
    let given = given.trim();

    if let Ok(bytes) = BASE64.decode(given)
        && bytes.len() == LENGTH
    {
        return Ok(bytes);
    }

    if given.len() == LENGTH * 2
        && let Some(bytes) = from_hex(given)
    {
        return Ok(bytes);
    }

    Err(format!(
        "{KEY} is {LENGTH} bytes, written as base64 or hex: \
         openssl rand -base64 {LENGTH}"
    ))
}

fn from_hex(given: &str) -> Option<Vec<u8>> {
    let digits: Vec<char> = given.chars().collect();
    let mut bytes = Vec::with_capacity(LENGTH);

    for pair in digits.chunks(2) {
        let pair: String = pair.iter().collect();
        bytes.push(u8::from_str_radix(&pair, 16).ok()?);
    }

    Some(bytes)
}

fn kept(at: &Path) -> Result<Vec<u8>, String> {
    if let Ok(held) = std::fs::read(at) {
        return read(&String::from_utf8_lossy(&held));
    }

    let mut key = [0u8; LENGTH];
    getrandom::fill(&mut key).map_err(|error| format!("no randomness for a key: {error}"))?;
    let written = BASE64.encode(key);

    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(at, &written).map_err(|error| format!("{}: {error}", at.display()))?;
    owner_only(at)?;

    tracing::info!(at = %at.display(), "made a key for the secrets this server keeps");
    Ok(key.to_vec())
}

#[cfg(unix)]
fn owner_only(at: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("{}: {error}", at.display()))
}

#[cfg(not(unix))]
fn owner_only(_at: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keeper() -> Keeper {
        Keeper::of(&[7u8; LENGTH]).expect("a key")
    }

    fn whose() -> Whose {
        Whose::new("cloud", "e2b", "api_key")
    }

    #[test]
    fn test_a_sealed_secret_comes_back_as_it_went_in() {
        let keeper = keeper();
        let secret = Secret::new("e2b_live_abcdef0123456789").expect("a secret");

        let sealed = keeper.seal(&whose(), &secret).expect("sealed");
        assert!(
            !sealed.as_str().contains("e2b_live_abcdef0123456789"),
            "the store holds nothing a reader could use"
        );

        assert_eq!(
            keeper.open(&whose(), &sealed).expect("opened").expose(),
            "e2b_live_abcdef0123456789"
        );
    }

    #[test]
    fn test_the_same_secret_seals_differently_every_time() {
        let keeper = keeper();
        let secret = Secret::new("e2b_live_abcdef0123456789").expect("a secret");

        assert_ne!(
            keeper.seal(&whose(), &secret).expect("sealed").as_str(),
            keeper.seal(&whose(), &secret).expect("sealed").as_str(),
            "two rows holding one key must not look alike"
        );
    }

    #[test]
    fn test_a_secret_moved_to_another_runtime_does_not_open() {
        let keeper = keeper();
        let sealed = keeper
            .seal(
                &whose(),
                &Secret::new("e2b_live_abcdef0123456789").expect("a secret"),
            )
            .expect("sealed");

        for elsewhere in [
            Whose::new("other", "e2b", "api_key"),
            Whose::new("cloud", "daytona", "api_key"),
            Whose::new("cloud", "e2b", "token"),
        ] {
            assert!(
                keeper.open(&elsewhere, &sealed).is_err(),
                "a row copied into another runtime's place must not open"
            );
        }
    }

    #[test]
    fn test_another_server_s_key_does_not_open_it() {
        let sealed = keeper()
            .seal(
                &whose(),
                &Secret::new("e2b_live_abcdef0123456789").expect("a secret"),
            )
            .expect("sealed");

        let theirs = Keeper::of(&[9u8; LENGTH]).expect("a key");

        assert!(
            theirs.open(&whose(), &sealed).is_err(),
            "a copy of the database is not a copy of the account"
        );
    }

    #[test]
    fn test_a_changed_byte_is_refused_rather_than_read() {
        let keeper = keeper();
        let sealed = keeper
            .seal(
                &whose(),
                &Secret::new("e2b_live_abcdef0123456789").expect("a secret"),
            )
            .expect("sealed");

        let mut bytes = BASE64.decode(sealed.as_str()).expect("base64");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;

        assert!(
            keeper
                .open(&whose(), &Sealed::of(BASE64.encode(bytes)))
                .is_err()
        );
    }

    #[test]
    fn test_a_server_with_no_key_keeps_no_secrets() {
        let keeper = Keeper::default();

        let Err(why) = keeper.seal(
            &whose(),
            &Secret::new("e2b_live_abcdef0123456789").expect("a secret"),
        ) else {
            panic!("a secret was stored in the clear");
        };
        assert!(why.contains(KEY), "the refusal says what to set: {why}");
    }

    #[test]
    fn test_a_key_is_read_as_base64_or_as_hex() {
        assert!(Keeper::of(&read(&BASE64.encode([3u8; LENGTH])).expect("base64")).is_ok());
        assert!(Keeper::of(&read(&"ab".repeat(LENGTH)).expect("hex")).is_ok());
        assert!(read("too short").is_err());
    }

    #[test]
    fn test_a_key_file_is_made_once_and_read_after() {
        let at = std::env::temp_dir().join(format!("computer-key-{}", std::process::id()));
        let _ = std::fs::remove_file(&at);

        let made = kept(&at).expect("a key");
        let read_again = kept(&at).expect("the same key");

        assert_eq!(
            made, read_again,
            "a restart must open what it sealed before"
        );
        let _ = std::fs::remove_file(&at);
    }
}
