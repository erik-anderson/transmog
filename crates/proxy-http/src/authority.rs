use std::{fmt, net::IpAddr, str::FromStr};

use thiserror::Error;

/// Validated authority-form target from an HTTP CONNECT request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectAuthority {
    host: String,
    port: u16,
}

impl ConnectAuthority {
    /// Host without IPv6 brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Required explicit port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the host as an IP literal when applicable.
    pub fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }
}

impl FromStr for ConnectAuthority {
    type Err = AuthorityError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.is_empty() || input.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(AuthorityError::Malformed);
        }
        let parsed =
            http::uri::Authority::from_str(input).map_err(|_| AuthorityError::Malformed)?;
        let port = parsed.port_u16().ok_or(AuthorityError::MissingPort)?;
        if port == 0 {
            return Err(AuthorityError::InvalidPort);
        }
        // `http::uri::Authority::host` currently retains brackets around IPv6
        // literals. Keep the stored host canonical so it is suitable for SNI,
        // certificate SAN matching, and socket address construction.
        let raw_host = parsed.host();
        let host = raw_host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(raw_host);
        if host.is_empty() || host.ends_with('.') {
            return Err(AuthorityError::Malformed);
        }
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }
}

impl fmt::Display for ConnectAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(formatter, "[{}]:{}", self.host, self.port)
        } else {
            write!(formatter, "{}:{}", self.host, self.port)
        }
    }
}

/// CONNECT authority validation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AuthorityError {
    /// Authority is not syntactically valid or uses a trailing-dot ambiguity.
    #[error("malformed CONNECT authority")]
    Malformed,
    /// CONNECT requires an explicit port.
    #[error("CONNECT authority is missing a port")]
    MissingPort,
    /// Port zero is never a valid origin target.
    #[error("CONNECT authority contains an invalid port")]
    InvalidPort,
}

#[cfg(test)]
mod tests {
    use super::{AuthorityError, ConnectAuthority};

    #[test]
    fn parses_dns_and_ipv6_authorities() {
        let dns: ConnectAuthority = "Example.COM:443".parse().unwrap();
        assert_eq!(dns.host(), "example.com");
        assert_eq!(dns.to_string(), "example.com:443");

        let ipv6: ConnectAuthority = "[::1]:8443".parse().unwrap();
        assert_eq!(ipv6.host(), "::1");
        assert_eq!(ipv6.to_string(), "[::1]:8443");
    }

    #[test]
    fn rejects_missing_zero_and_ambiguous_ports() {
        assert_eq!(
            "example.com".parse::<ConnectAuthority>(),
            Err(AuthorityError::MissingPort)
        );
        assert_eq!(
            "example.com:0".parse::<ConnectAuthority>(),
            Err(AuthorityError::InvalidPort)
        );
        assert!("example.com:443:80".parse::<ConnectAuthority>().is_err());
    }
}
