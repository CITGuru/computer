use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Bind {
    #[default]
    Loopback,
    Any,
    Address(IpAddr),
}

impl Bind {
    pub fn publish_prefix(&self) -> String {
        match self {
            Self::Loopback => "127.0.0.1".to_string(),
            Self::Any => "0.0.0.0".to_string(),
            Self::Address(address) => address.to_string(),
        }
    }

    pub fn reach(&self) -> Reach {
        match self {
            Self::Loopback => Reach::Loopback,
            Self::Any => Reach::Routable,
            Self::Address(address) if address.is_loopback() => Reach::Loopback,
            Self::Address(_) => Reach::Routable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reach {
    #[default]
    Loopback,
    Routable,
}

impl Reach {
    pub fn needs_a_secret(&self) -> bool {
        matches!(self, Self::Routable)
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
}

impl Address {
    pub fn loopback(port: u16) -> Self {
        Self {
            scheme: Scheme::Http,
            host: "127.0.0.1".to_string(),
            port,
        }
    }

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
