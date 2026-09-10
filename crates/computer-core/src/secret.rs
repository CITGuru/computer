//! A value that must not be guessed, and must not be printed.
//!
//! Everything that gates a door in this crate is one of these: the takeover
//! token today, the viewer credential next. They share a type because they
//! share the two properties that make them work — enough entropy that guessing
//! is not a strategy, and no path onto a terminal or into a log.

use crate::{Error, Result};

/// How many bytes a minted secret carries.
///
/// 256 bits: the amount at which guessing stops being a consideration and the
/// only remaining question is whether the value leaked.
const BYTES: usize = 32;

/// The shortest value a caller may supply.
///
/// A caller passing their own is trusted about its provenance but not about its
/// size — a four-character password on a routable viewer is the failure this
/// whole design exists to prevent, and it is worth refusing loudly rather than
/// gating a desktop with it.
const MIN_SUPPLIED: usize = 16;

/// A credential, carried without a way to print it by accident.
///
/// [`Debug`] redacts, and there is no [`Display`](std::fmt::Display) and no
/// `Serialize`. The value leaves through [`expose`](Secret::expose) alone,
/// which is named to be conspicuous at the call site and in review.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// 256 bits from the operating system.
    ///
    /// Fails only where the OS has no random source to give, which is a system
    /// too broken to hand out desktops on — so this is reported rather than
    /// papered over with the clock.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; BYTES];
        getrandom::fill(&mut bytes).map_err(|error| Error::Unavailable {
            runtime: "the system random source".to_string(),
            detail: error.to_string(),
        })?;

        let mut hex = String::with_capacity(BYTES * 2);
        for byte in bytes {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
        Ok(Self(hex))
    }

    /// A value the caller already holds.
    ///
    /// For a secret that has to outlive the process: a second program attaching
    /// to the same box needs the same value, the same way it needs the same
    /// profile.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() < MIN_SUPPLIED {
            return Err(Error::denied(format!(
                "a secret of {} characters is too short to gate a desktop: \
                 {MIN_SUPPLIED} is the minimum, and `Secret::generate` mints \
                 {} bits",
                value.len(),
                BYTES * 8
            )));
        }
        Ok(Self(value))
    }

    /// A value from the environment, which is where a deployment usually keeps
    /// one.
    ///
    /// The variable's *name* appears in the refusal and its value never does.
    pub fn from_env(key: &str) -> Result<Self> {
        let value = std::env::var(key).map_err(|_| {
            Error::denied(format!(
                "{key} is not set, and it is where the secret was expected"
            ))
        })?;
        Self::new(value)
    }

    /// The value itself, for the one place it has to go: into the box.
    ///
    /// Every call is a place the secret can escape from. There are meant to be
    /// few, and they are meant to be easy to find.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl PartialEq for Secret {
    /// Constant time in the length the two share.
    ///
    /// Nothing in this crate compares secrets across a network yet, so this is
    /// not answering a live attack — it is making the type safe to use in one
    /// later, rather than leaving a `==` that has to be remembered.
    fn eq(&self, other: &Self) -> bool {
        let (ours, theirs) = (self.0.as_bytes(), other.0.as_bytes());
        if ours.len() != theirs.len() {
            return false;
        }
        ours.iter()
            .zip(theirs)
            .fold(0u8, |differing, (a, b)| differing | (a ^ b))
            == 0
    }
}

impl Eq for Secret {}

impl std::fmt::Debug for Secret {
    /// Never the value.
    ///
    /// A secret reaches a log through `{:?}` on the struct that holds it, which
    /// is a line nobody writes deliberately. The length is safe and says enough
    /// to tell "unset" from "set to the wrong thing".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret(redacted, {} chars)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_minted_secret_carries_the_full_width() {
        let secret = Secret::generate().expect("a system with a random source");
        assert_eq!(
            secret.expose().len(),
            BYTES * 2,
            "256 bits, hex encoded, is 64 characters"
        );
        assert!(secret.expose().chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The whole point of the type: two mints must not collide.
    #[test]
    fn test_two_mints_do_not_agree() {
        let (one, two) = (
            Secret::generate().expect("random"),
            Secret::generate().expect("random"),
        );
        assert_ne!(one, two, "a CSPRNG that repeats is not one");
    }

    /// The failure this replaces: the old takeover token was the clock and a
    /// counter, so a second caller could work out the first one's.
    #[test]
    fn test_a_mint_is_not_derived_from_the_clock() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            assert!(
                seen.insert(Secret::generate().expect("random").expose().to_string()),
                "a repeat inside one loop means this is not random"
            );
        }
    }

    #[test]
    fn test_a_short_supplied_secret_is_refused() {
        let error = Secret::new("hunter2").expect_err("seven characters is not a secret");
        assert!(matches!(error, Error::Denied { .. }));
        assert!(
            !error.to_string().contains("hunter2"),
            "the refusal must not quote the value it refused"
        );
    }

    #[test]
    fn test_a_supplied_secret_of_length_is_kept_verbatim() {
        let secret = Secret::new("a-secret-from-a-vault").expect("long enough");
        assert_eq!(secret.expose(), "a-secret-from-a-vault");
    }

    /// A secret that prints itself is one that reaches a log.
    #[test]
    fn test_debug_never_shows_the_value() {
        let secret = Secret::new("correct-horse-battery-staple").expect("long enough");
        let shown = format!("{secret:?}");
        assert!(!shown.contains("correct-horse"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");

        // The common way one escapes: `{:?}` on the struct that carries it.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            secret: Secret,
        }
        let nested = format!("{:?}", Holder { secret });
        assert!(!nested.contains("correct-horse"), "{nested}");
    }

    #[test]
    fn test_equality_holds_and_rejects() {
        let secret = Secret::new("a-secret-from-a-vault").expect("long enough");
        assert_eq!(secret, secret.clone());
        assert_ne!(
            secret,
            Secret::new("a-secret-from-a-vaulX").expect("long enough"),
            "one character apart is a different secret"
        );
        assert_ne!(
            secret,
            Secret::new("a-secret-from-a-vault-and-more").expect("long enough"),
            "a prefix is not a match"
        );
    }

    #[test]
    fn test_an_unset_variable_names_itself_and_nothing_else() {
        let error = Secret::from_env("COMPUTER_TEST_SECRET_THAT_IS_NOT_SET")
            .expect_err("the variable is not set");
        assert!(
            error
                .to_string()
                .contains("COMPUTER_TEST_SECRET_THAT_IS_NOT_SET")
        );
    }
}
