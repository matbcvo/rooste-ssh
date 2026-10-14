//! Core session model shared by every rooste-ssh frontend.

use std::fmt;
use std::str::FromStr;

/// Connection protocol of a saved session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Protocol {
    #[default]
    Ssh,
    Serial,
    Telnet,
    Raw,
}

impl Protocol {
    pub const ALL: [Protocol; 4] = [Self::Ssh, Self::Serial, Self::Telnet, Self::Raw];

    /// Name used in configuration files.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Serial => "serial",
            Self::Telnet => "telnet",
            Self::Raw => "raw",
        }
    }

    /// Default TCP port, or `None` for protocols that are not TCP based.
    pub fn default_port(self) -> Option<u16> {
        match self {
            Self::Ssh => Some(22),
            Self::Telnet => Some(23),
            Self::Serial | Self::Raw => None,
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown protocol name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownProtocol(pub String);

impl fmt::Display for UnknownProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown protocol `{}`", self.0)
    }
}

impl std::error::Error for UnknownProtocol {}

impl FromStr for Protocol {
    type Err = UnknownProtocol;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| UnknownProtocol(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_through_string() {
        for p in Protocol::ALL {
            assert_eq!(p.to_string().parse::<Protocol>(), Ok(p));
        }
    }

    #[test]
    fn protocol_parse_is_case_insensitive() {
        assert_eq!(" SSH ".parse::<Protocol>(), Ok(Protocol::Ssh));
    }

    #[test]
    fn protocol_parse_rejects_unknown() {
        assert_eq!(
            "rlogin".parse::<Protocol>(),
            Err(UnknownProtocol("rlogin".into()))
        );
    }

    #[test]
    fn default_ports() {
        assert_eq!(Protocol::default().default_port(), Some(22));
        assert_eq!(Protocol::Telnet.default_port(), Some(23));
        assert_eq!(Protocol::Serial.default_port(), None);
    }
}
