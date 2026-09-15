use crate::Secret;
use crate::error::Result;

pub const AUTH_ENV: &str = "COMPUTER_VIEWER_AUTH";
pub const VIEW_SECRET_ENV: &str = "COMPUTER_VIEW_SECRET";
pub const CONTROL_SECRET_ENV: &str = "COMPUTER_CONTROL_SECRET";

pub const VIEWER_USER: &str = "computer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Auth {
    #[default]
    Open,
    Password,
    Token,
}

impl Auth {
    pub fn is_gated(&self) -> bool {
        !matches!(self, Self::Open)
    }

    pub fn is_in_the_url(&self) -> bool {
        matches!(self, Self::Token)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Password => "password",
            Self::Token => "token",
        }
    }
}

/// Separate values: the viewers differ only by port, so one would open control to every watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub view: Secret,
    pub control: Secret,
}

impl Credentials {
    pub fn generate() -> Result<Self> {
        Ok(Self {
            view: Secret::generate()?,
            control: Secret::generate()?,
        })
    }

    pub fn new(view: Secret, control: Secret) -> Result<Self> {
        if view == control {
            return Err(crate::Error::denied(
                "the viewer and control credentials are the same value, which \
                 makes every read-only URL a control URL one port along",
            ));
        }
        Ok(Self { view, control })
    }
}

pub fn from_environment(
    environment: &std::collections::BTreeMap<String, String>,
) -> (Auth, Option<Credentials>) {
    let auth = match environment.get(AUTH_ENV).map(String::as_str) {
        Some("password") => Auth::Password,
        Some("token") => Auth::Token,
        _ => Auth::Open,
    };

    let pair = environment
        .get(VIEW_SECRET_ENV)
        .zip(environment.get(CONTROL_SECRET_ENV))
        .and_then(|(view, control)| {
            Credentials::new(Secret::new(view).ok()?, Secret::new(control).ok()?).ok()
        });

    (auth, pair)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_is_the_default_and_the_only_ungated_one() {
        assert_eq!(Auth::default(), Auth::Open);
        assert!(!Auth::Open.is_gated());
        assert!(Auth::Password.is_gated());
        assert!(Auth::Token.is_gated());
    }

    #[test]
    fn test_only_a_token_travels_in_the_url() {
        assert!(Auth::Token.is_in_the_url());
        assert!(!Auth::Password.is_in_the_url());
        assert!(!Auth::Open.is_in_the_url());
    }

    #[test]
    fn test_the_image_reads_a_word_it_can_case_on() {
        assert_eq!(Auth::Open.as_str(), "open");
        assert_eq!(Auth::Password.as_str(), "password");
        assert_eq!(Auth::Token.as_str(), "token");
    }

    #[test]
    fn test_a_minted_pair_does_not_share_a_value() {
        let pair = Credentials::generate().expect("a system with a random source");
        assert_ne!(
            pair.view, pair.control,
            "one value across both doors makes a watch link a control link"
        );
    }

    #[test]
    fn test_the_same_value_twice_is_refused() {
        let secret = Secret::new("a-secret-from-a-vault").expect("long enough");
        let error = Credentials::new(secret.clone(), secret)
            .expect_err("one value across both doors is not two credentials");

        assert!(matches!(error, crate::Error::Denied { .. }));
        assert!(
            !error.to_string().contains("a-secret-from-a-vault"),
            "the refusal must not quote what it refused"
        );
    }

    #[test]
    fn test_two_different_values_are_kept() {
        let pair = Credentials::new(
            Secret::new("a-secret-from-a-vault").expect("long enough"),
            Secret::new("another-secret-entirely").expect("long enough"),
        )
        .expect("two different values");

        assert_eq!(pair.view.expose(), "a-secret-from-a-vault");
        assert_eq!(pair.control.expose(), "another-secret-entirely");
    }
}
