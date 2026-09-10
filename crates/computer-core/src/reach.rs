//! Where a box can be reached from.
//!
//! Three things that are easy to conflate and must not be: which addresses a
//! port is *published* on ([`Bind`]), whether that puts it beyond this host
//! ([`Reach`]), and the address a person is *told* to use ([`Address`]).
//!
//! They come apart in practice. A box published on every interface is reachable
//! at a name this crate has never been told, so the address in a URL cannot be
//! derived from the bind — it has to be supplied. And a [`Machine`] that
//! forwards nothing, like a sandbox that publishes a hostname per port, has no
//! bind at all and still answers from the internet.
//!
//! [`Machine`]: crate::Machine

use std::net::IpAddr;

/// Which addresses a published port answers on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Bind {
    /// 127.0.0.1. Nothing off this host connects.
    #[default]
    Loopback,
    /// Every interface. Whoever can route to this host reaches the box.
    Any,
    /// One interface, named.
    Address(IpAddr),
}

impl Bind {
    /// The host side of a `--publish`, as a container runtime wants it.
    pub fn publish_prefix(&self) -> String {
        match self {
            Self::Loopback => "127.0.0.1".to_string(),
            Self::Any => "0.0.0.0".to_string(),
            Self::Address(address) => address.to_string(),
        }
    }

    /// Whether this hands out an address beyond the host.
    ///
    /// Loopback is loopback however it is spelled: an explicit `127.0.0.1` or
    /// `::1` answers the same as [`Bind::Loopback`], because refusing it would
    /// send a caller looking for a credential they do not need.
    pub fn reach(&self) -> Reach {
        match self {
            Self::Loopback => Reach::Loopback,
            Self::Any => Reach::Routable,
            Self::Address(address) if address.is_loopback() => Reach::Loopback,
            Self::Address(_) => Reach::Routable,
        }
    }
}

/// Whether what a machine published can be reached beyond this host.
///
/// [`Reach::Loopback`] is not a claim that nothing else can get in. It is a
/// claim that this crate did not hand out the address — which is the only thing
/// this crate is in a position to promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reach {
    /// This host only, as far as this crate arranged.
    #[default]
    Loopback,
    /// Somewhere else can connect.
    Routable,
}

impl Reach {
    pub fn needs_a_secret(&self) -> bool {
        matches!(self, Self::Routable)
    }
}

/// How a URL addresses the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scheme {
    #[default]
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// Where a published port answers, as a person is told it.
///
/// The host is carried rather than derived: a box on every interface is reached
/// at whatever name resolves to this machine, and nothing here knows that name
/// unless a caller says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
}

impl Address {
    /// The loopback address, which is where every box is until told otherwise.
    pub fn loopback(port: u16) -> Self {
        Self {
            scheme: Scheme::Http,
            host: "127.0.0.1".to_string(),
            port,
        }
    }

    /// The authority a URL is built on.
    ///
    /// IPv6 needs the brackets, and a literal without them produces a URL that
    /// parses as a different host and port entirely.
    pub fn authority(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_loopback_keeps_the_address_on_this_host() {
        assert_eq!(Bind::Loopback.reach(), Reach::Loopback);
        assert_eq!(Bind::Any.reach(), Reach::Routable);
        assert_eq!(
            Bind::Address("192.168.1.4".parse().unwrap()).reach(),
            Reach::Routable
        );
    }

    /// `127.0.0.1` written out is still loopback, and refusing it would send a
    /// caller looking for a secret they do not need.
    #[test]
    fn test_loopback_spelled_out_is_still_loopback() {
        assert_eq!(
            Bind::Address("127.0.0.1".parse().unwrap()).reach(),
            Reach::Loopback
        );
        assert_eq!(
            Bind::Address("::1".parse().unwrap()).reach(),
            Reach::Loopback
        );
    }

    #[test]
    fn test_the_default_is_the_safe_one() {
        assert_eq!(Bind::default(), Bind::Loopback);
        assert_eq!(Reach::default(), Reach::Loopback);
        assert!(!Reach::default().needs_a_secret());
        assert!(Reach::Routable.needs_a_secret());
    }

    #[test]
    fn test_the_publish_prefix_is_what_a_runtime_wants() {
        assert_eq!(Bind::Loopback.publish_prefix(), "127.0.0.1");
        assert_eq!(Bind::Any.publish_prefix(), "0.0.0.0");
        assert_eq!(
            Bind::Address("10.0.0.7".parse().unwrap()).publish_prefix(),
            "10.0.0.7"
        );
    }

    /// A v6 literal without brackets is a URL that means something else.
    #[test]
    fn test_an_ipv6_host_is_bracketed() {
        let address = Address {
            scheme: Scheme::Http,
            host: "::1".to_string(),
            port: 6080,
        };
        assert_eq!(address.authority(), "[::1]:6080");
    }

    #[test]
    fn test_a_name_is_left_alone() {
        let address = Address {
            scheme: Scheme::Https,
            host: "boxes.example.com".to_string(),
            port: 443,
        };
        assert_eq!(address.authority(), "boxes.example.com:443");
        assert_eq!(address.scheme.as_str(), "https");
    }
}
