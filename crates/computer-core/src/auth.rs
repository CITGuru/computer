//! The gate in front of a viewer, and the credentials it reads.
//!
//! Two shapes, because they fail in opposite directions and a deployment knows
//! which one it can live with. [`Auth::Password`] keeps the credential out of
//! every URL and cannot be put in a link; [`Auth::Token`] puts it in the link
//! and therefore in whatever the link touches.
//!
//! [`Auth::Open`] is the default and is what every box on loopback has always
//! been. It is refused wherever the port is reachable beyond this host.

use crate::Secret;
use crate::error::Result;

/// What `screen.sh` is told the gate is.
pub const AUTH_ENV: &str = "COMPUTER_VIEWER_AUTH";
/// Where the read-only viewer's credential travels.
pub const VIEW_SECRET_ENV: &str = "COMPUTER_VIEW_SECRET";
/// Where the control viewer's credential travels.
pub const CONTROL_SECRET_ENV: &str = "COMPUTER_CONTROL_SECRET";

/// The user half of the browser prompt.
///
/// Fixed, because it is not a second secret and a caller who has to tell a
/// person two things will tell them the wrong one. The password carries the
/// entropy.
pub const VIEWER_USER: &str = "computer";

/// How a viewer asks who is connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Auth {
    /// Nothing asks.
    ///
    /// Whoever reaches the port gets the desktop, and on the control port they
    /// drive it. Refused wherever [`crate::Reach`] says the port is reachable
    /// beyond this host.
    #[default]
    Open,
    /// A browser prompt, and the noVNC page gated with the socket.
    ///
    /// The credential reaches the browser through the prompt, so it lands in no
    /// history, no referrer header and no proxy log. The cost is that there is
    /// no link to hand anybody: the address and the password travel separately,
    /// and revoking means restarting the viewer.
    Password,
    /// A ticket in the URL's query.
    ///
    /// One link carries everything, which is the only shape that can be sent to
    /// a person who is not going to be talked through a login. The cost is that
    /// the credential goes wherever the link goes — browser history, the
    /// referrer on anything the desktop opens, and the access log of every
    /// proxy in front. The box's deadline is what bounds it.
    Token,
}

impl Auth {
    pub fn is_gated(&self) -> bool {
        !matches!(self, Self::Open)
    }

    pub fn is_in_the_url(&self) -> bool {
        matches!(self, Self::Token)
    }

    /// The word the image reads out of the environment.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Password => "password",
            Self::Token => "token",
        }
    }
}

/// The credentials for one box's two doors.
///
/// Separate values, and not one across both: the read-only viewer and the
/// control viewer differ by a port number in a URL, so a single credential
/// would make every watch link a control link for anybody who tried the next
/// port. The takeover token does not close that gap — `input-guard.sh` shadows
/// `xdotool`, and a person on the control port drives over VNC without going
/// near it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub view: Secret,
    pub control: Secret,
}

impl Credentials {
    /// A fresh pair from the CSPRNG.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            view: Secret::generate()?,
            control: Secret::generate()?,
        })
    }

    /// A pair a caller already holds.
    ///
    /// For credentials that have to outlive the process: a second program
    /// attaching to the same box hands out the same URLs only if it was given
    /// the same values.
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

/// The gate a running box carries, read back out of its environment.
///
/// A box outlives the process that made it, so what its viewers ask cannot
/// live only in that process's memory.
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

    /// Which one leaks into a URL decides what `viewer_url` may append, so it
    /// is asked rather than matched on at every call site.
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

    /// The mistake a caller supplying their own is most likely to make.
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
