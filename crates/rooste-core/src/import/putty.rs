//! PuTTY saved sessions: the Windows registry
//! (`HKCU\Software\SimonTatham\PuTTY\Sessions`) or `~/.putty/sessions` on
//! Linux, where each session is a file of `Key=Value` lines.

use std::io;
use std::path::Path;

use crate::profile::{FlowControl, Parity, SerialSettings};
use crate::{Profile, Protocol};

/// Folder imported sessions are placed in.
pub const FOLDER: &str = "Imported/PuTTY";

/// Registry path of PuTTY's sessions under `HKEY_CURRENT_USER`.
pub const REGISTRY_PATH: &str = r"Software\SimonTatham\PuTTY\Sessions";

/// Build a profile from a session's settings. `get` looks up a setting by
/// PuTTY's name. Returns `None` for sessions that cannot be imported (no
/// host, the "Default Settings" template, unsupported protocols).
pub fn profile_from_settings(name: &str, get: impl Fn(&str) -> Option<String>) -> Option<Profile> {
    let name = decode_session_name(name);
    if name == "Default Settings" {
        return None;
    }
    let protocol = match get("Protocol").as_deref().unwrap_or("ssh") {
        "ssh" => Protocol::Ssh,
        "telnet" => Protocol::Telnet,
        "raw" => Protocol::Raw,
        "serial" => Protocol::Serial,
        _ => return None,
    };
    let host = if protocol == Protocol::Serial {
        get("SerialLine")?
    } else {
        get("HostName")?
    };
    let host = host.trim().to_owned();
    if host.is_empty() {
        return None;
    }
    // PuTTY allows `user@host` in the host field.
    let (user_in_host, host) = match host.rsplit_once('@') {
        Some((user, host)) => (Some(user.to_owned()), host.to_owned()),
        None => (None, host),
    };
    let port = get("PortNumber")
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|&p| p != 0 && Some(p) != protocol.default_port());
    let key_file = get("PublicKeyFile")
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .map(Into::into);
    let number = |key: &str| get(key).and_then(|v| v.trim().parse::<u32>().ok());
    let defaults = SerialSettings::default();
    let serial = SerialSettings {
        baud: number("SerialSpeed")
            .filter(|&b| b > 0)
            .unwrap_or(defaults.baud),
        data_bits: number("SerialDataBits")
            .and_then(|b| u8::try_from(b).ok())
            .filter(|b| (5..=8).contains(b))
            .unwrap_or(defaults.data_bits),
        // PuTTY counts stop bits in halves: 2 = one, 4 = two.
        stop_bits: if number("SerialStopHalfbits") == Some(4) {
            2
        } else {
            1
        },
        // PuTTY: 0 none, 1 odd, 2 even, 3 mark, 4 space (mark/space unsupported).
        parity: match number("SerialParity") {
            Some(1) => Parity::Odd,
            Some(2) => Parity::Even,
            _ => Parity::None,
        },
        // PuTTY: 0 none, 1 XON/XOFF, 2 RTS/CTS, 3 DSR/DTR (treated as RTS/CTS).
        flow_control: match number("SerialFlowControl") {
            Some(1) => FlowControl::XonXoff,
            Some(2) | Some(3) => FlowControl::RtsCts,
            _ => FlowControl::None,
        },
    };
    Some(Profile {
        name,
        folder: FOLDER.into(),
        protocol,
        host,
        port: if protocol == Protocol::Serial {
            None
        } else {
            port
        },
        user: get("UserName")
            .filter(|u| !u.is_empty())
            .or(user_in_host)
            .unwrap_or_default(),
        key_file,
        serial,
        notes: "Imported from PuTTY".into(),
        ..Profile::default()
    })
}

/// Parse one session file from `~/.putty/sessions`.
pub fn parse_session_file(file_name: &str, contents: &str) -> Option<Profile> {
    let settings: Vec<(&str, &str)> = contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    profile_from_settings(file_name, |key| {
        settings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    })
}

/// Import every session file in a `~/.putty/sessions`-style directory.
pub fn import_dir(dir: &Path) -> io::Result<Vec<Profile>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let contents = std::fs::read_to_string(entry.path()).ok()?;
            parse_session_file(&name, &contents)
        })
        .collect())
}

/// Import from the registry key `path` under `HKEY_CURRENT_USER`.
#[cfg(windows)]
pub fn import_registry(path: &str) -> io::Result<Vec<Profile>> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let sessions = RegKey::predef(HKEY_CURRENT_USER).open_subkey(path)?;
    let mut names: Vec<String> = sessions.enum_keys().filter_map(Result::ok).collect();
    names.sort();
    Ok(names
        .into_iter()
        .filter_map(|name| {
            let key = sessions.open_subkey(&name).ok()?;
            profile_from_settings(&name, |value| {
                key.get_value::<String, _>(value)
                    .ok()
                    .or_else(|| key.get_value::<u32, _>(value).ok().map(|n| n.to_string()))
            })
        })
        .collect())
}

/// Import PuTTY's sessions from their standard location on this platform.
pub fn import_default() -> io::Result<Vec<Profile>> {
    #[cfg(windows)]
    {
        import_registry(REGISTRY_PATH)
    }
    #[cfg(not(windows))]
    {
        let home = std::env::home_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
        import_dir(&home.join(".putty").join("sessions"))
    }
}

/// PuTTY stores session names percent-encoded (`My%20Server`).
fn decode_session_name(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&name[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "\
HostName=web.example.com
PortNumber=2222
Protocol=ssh
UserName=deploy
PublicKeyFile=/home/me/keys/web.ppk
TerminalType=xterm
";

    #[test]
    fn parses_a_session_file() {
        let p = parse_session_file("Web%20Server", SESSION).unwrap();
        assert_eq!(p.name, "Web Server");
        assert_eq!(p.folder, FOLDER);
        assert_eq!(p.host, "web.example.com");
        assert_eq!(p.port, Some(2222));
        assert_eq!(p.user, "deploy");
        assert_eq!(p.protocol, Protocol::Ssh);
        assert_eq!(
            p.key_file.as_deref(),
            Some(Path::new("/home/me/keys/web.ppk"))
        );
    }

    #[test]
    fn default_port_and_user_in_host() {
        let p = parse_session_file("x", "HostName=root@10.0.0.1\nPortNumber=22\n").unwrap();
        assert_eq!(p.host, "10.0.0.1");
        assert_eq!(p.user, "root");
        assert_eq!(p.port, None, "the protocol default is not stored");
    }

    #[test]
    fn telnet_and_serial_sessions() {
        let t =
            parse_session_file("sw", "HostName=switch\nProtocol=telnet\nPortNumber=23\n").unwrap();
        assert_eq!(t.protocol, Protocol::Telnet);
        assert_eq!(t.port, None);

        let s = parse_session_file(
            "console",
            "Protocol=serial\nSerialLine=COM3\nSerialSpeed=115200\nSerialDataBits=7\n\
             SerialStopHalfbits=4\nSerialParity=2\nSerialFlowControl=1\n",
        )
        .unwrap();
        assert_eq!(s.protocol, Protocol::Serial);
        assert_eq!(s.host, "COM3");
        assert_eq!(s.serial.describe(), "115200 7E2");
        assert_eq!(s.serial.flow_control, FlowControl::XonXoff);
    }

    #[test]
    fn skips_templates_and_unsupported() {
        assert!(parse_session_file("Default%20Settings", "HostName=x\n").is_none());
        assert!(parse_session_file("empty", "HostName=\n").is_none());
        assert!(parse_session_file("old", "HostName=x\nProtocol=rlogin\n").is_none());
    }

    #[test]
    fn name_decoding() {
        assert_eq!(decode_session_name("a%20b%2Fc"), "a b/c");
        assert_eq!(decode_session_name("%C3%B5ige"), "õige");
        assert_eq!(decode_session_name("100%"), "100%");
        assert_eq!(decode_session_name("%zz"), "%zz");
    }

    #[test]
    fn imports_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Web%20Server"), SESSION).unwrap();
        std::fs::write(dir.path().join("Default%20Settings"), "HostName=\n").unwrap();
        let profiles = import_dir(dir.path()).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Web Server");
    }

    #[cfg(windows)]
    #[test]
    fn imports_from_the_registry() {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;

        let root = format!(r"Software\rooste-ssh-test-{}", std::process::id());
        let path = format!(r"{root}\Sessions");
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (session, _) = hkcu.create_subkey(format!(r"{path}\Router%20A")).unwrap();
        session.set_value("HostName", &"192.168.1.1").unwrap();
        session.set_value("PortNumber", &2200u32).unwrap();
        session.set_value("UserName", &"admin").unwrap();
        session.set_value("Protocol", &"ssh").unwrap();

        let profiles = import_registry(&path);
        hkcu.delete_subkey_all(&root).unwrap();
        let profiles = profiles.unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Router A");
        assert_eq!(profiles[0].host, "192.168.1.1");
        assert_eq!(profiles[0].port, Some(2200));
        assert_eq!(profiles[0].user, "admin");
    }
}
