//! Parsing `[user@]host[:port]`, including bracketed IPv6 addresses.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Why a destination could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DestinationError(String);

macro_rules! bail {
    ($($arg:tt)*) => { return Err(DestinationError(format!($($arg)*))) };
}

impl Destination {
    /// Parse a comma-separated jump host list, as OpenSSH `ProxyJump`.
    pub fn parse_list(input: &str) -> Result<Vec<Self>, DestinationError> {
        input
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Self::parse)
            .collect()
    }

    pub fn parse(input: &str) -> Result<Self, DestinationError> {
        let (user, rest) = match input.rsplit_once('@') {
            Some((user, rest)) if !user.is_empty() => (Some(user.to_owned()), rest),
            Some(_) => bail!("empty user name in `{input}`"),
            None => (None, input),
        };

        let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
            let (host, after) = bracketed
                .split_once(']')
                .ok_or_else(|| DestinationError(format!("missing `]` in `{input}`")))?;
            match after {
                "" => (host, None),
                _ => (host, Some(after.strip_prefix(':').unwrap_or(after))),
            }
        } else if rest.matches(':').count() == 1 {
            let (host, port) = rest.split_once(':').unwrap_or((rest, ""));
            (host, Some(port))
        } else {
            // Bare host, or an unbracketed IPv6 address.
            (rest, None)
        };

        if host.is_empty() {
            bail!("empty host name in `{input}`");
        }
        let port = port
            .map(|p| {
                p.parse::<u16>()
                    .map_err(|_| DestinationError(format!("invalid port `{p}`")))
            })
            .transpose()?;
        Ok(Self {
            user,
            host: host.to_owned(),
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Destination {
        Destination::parse(s).unwrap()
    }

    #[test]
    fn host_only() {
        assert_eq!(
            parse("example.com"),
            Destination {
                user: None,
                host: "example.com".into(),
                port: None
            }
        );
    }

    #[test]
    fn user_host_port() {
        let d = parse("me@example.com:2222");
        assert_eq!(d.user.as_deref(), Some("me"));
        assert_eq!(d.host, "example.com");
        assert_eq!(d.port, Some(2222));
    }

    #[test]
    fn user_may_contain_at() {
        let d = parse("me@corp.example@host");
        assert_eq!(d.user.as_deref(), Some("me@corp.example"));
        assert_eq!(d.host, "host");
    }

    #[test]
    fn ipv6() {
        assert_eq!(parse("::1").host, "::1");
        let d = parse("root@[fe80::1]:22");
        assert_eq!(d.host, "fe80::1");
        assert_eq!(d.port, Some(22));
        assert_eq!(parse("[::1]").port, None);
    }

    #[test]
    fn jump_lists() {
        let hops = Destination::parse_list("a@bastion:2222, inner").unwrap();
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].port, Some(2222));
        assert_eq!(hops[1].host, "inner");
        assert!(Destination::parse_list("").unwrap().is_empty());
    }

    #[test]
    fn errors() {
        assert!(Destination::parse("@host").is_err());
        assert!(Destination::parse("me@").is_err());
        assert!(Destination::parse("host:99999").is_err());
        assert!(Destination::parse("[::1").is_err());
    }
}
