//! X11 forwarding: remote graphical programs draw on the local X display.
//!
//! The server only ever sees a random fake cookie. Each forwarded
//! connection starts with an X11 setup request carrying that cookie; it is
//! checked and replaced with the local display's real cookie (from
//! `~/.Xauthority`) before the bytes reach the local X server, as OpenSSH
//! does.

use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The authentication protocol used with the server and the local display.
pub const COOKIE_PROTOCOL: &str = "MIT-MAGIC-COOKIE-1";

/// Where the local X server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11Target {
    /// A Unix socket such as `/tmp/.X11-unix/X0`.
    Unix(PathBuf),
    Tcp {
        host: String,
        port: u16,
    },
}

/// A cookie that the local X server accepts.
#[derive(Clone, PartialEq, Eq)]
pub struct X11Cookie {
    pub protocol: String,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for X11Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "X11Cookie({}, ..)", self.protocol)
    }
}

/// The local display to forward to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11Forwarding {
    pub target: X11Target,
    pub screen: u32,
    /// The real cookie, if the local X server needs one.
    pub cookie: Option<X11Cookie>,
}

impl X11Forwarding {
    /// The display in `$DISPLAY` (on Windows `localhost:0` if unset, for X
    /// servers such as VcXsrv), with its cookie from `$XAUTHORITY` or
    /// `~/.Xauthority`.
    pub fn from_environment() -> Result<Self, String> {
        let display = match std::env::var("DISPLAY") {
            Ok(display) if !display.is_empty() => display,
            _ if cfg!(windows) => "localhost:0".to_owned(),
            _ => return Err("there is no local X display (DISPLAY is not set)".into()),
        };
        let parsed =
            parse_display(&display).ok_or_else(|| format!("cannot use DISPLAY={display}"))?;
        let cookie = xauthority_path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|file| find_cookie(&file, parsed.number));
        Ok(Self {
            target: parsed.target,
            screen: parsed.screen,
            cookie,
        })
    }
}

/// A parsed `DISPLAY` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    pub target: X11Target,
    pub number: u32,
    pub screen: u32,
}

/// Parse `DISPLAY`: `:0`, `:0.0`, `unix:1`, `localhost:10.0`, `host:0`.
pub fn parse_display(display: &str) -> Option<Display> {
    let (host, rest) = display.rsplit_once(':')?;
    let (number, screen) = match rest.split_once('.') {
        Some((number, screen)) => (number.parse().ok()?, screen.parse().ok()?),
        None => (rest.parse().ok()?, 0),
    };
    let target = if host.is_empty() || host == "unix" {
        if cfg!(unix) {
            X11Target::Unix(PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
        } else {
            X11Target::Tcp {
                host: "localhost".into(),
                port: 6000u16.checked_add(u16::try_from(number).ok()?)?,
            }
        }
    } else {
        X11Target::Tcp {
            host: host.to_owned(),
            port: 6000u16.checked_add(u16::try_from(number).ok()?)?,
        }
    };
    Some(Display {
        target,
        number,
        screen,
    })
}

fn xauthority_path() -> Option<PathBuf> {
    match std::env::var_os("XAUTHORITY") {
        Some(path) if !path.is_empty() => Some(path.into()),
        _ => std::env::home_dir().map(|home| home.join(".Xauthority")),
    }
}

/// The MIT-MAGIC-COOKIE-1 cookie for display `number` in an Xauthority
/// file: entries are family (u16), then address, display number, protocol
/// name and data, each a big-endian u16 length and that many bytes.
pub fn find_cookie(file: &[u8], number: u32) -> Option<X11Cookie> {
    let number = number.to_string();
    let mut rest = file;
    let field = |rest: &mut &[u8]| -> Option<Vec<u8>> {
        let (len, tail) = rest.split_first_chunk::<2>()?;
        let len = usize::from(u16::from_be_bytes(*len));
        let (value, tail) = (tail.get(..len)?, tail.get(len..)?);
        *rest = tail;
        Some(value.to_vec())
    };
    while !rest.is_empty() {
        let (_family, tail) = rest.split_first_chunk::<2>()?;
        rest = tail;
        let _address = field(&mut rest)?;
        let display = field(&mut rest)?;
        let protocol = field(&mut rest)?;
        let data = field(&mut rest)?;
        // An empty display number matches any display.
        if (display.is_empty() || display == number.as_bytes())
            && protocol == COOKIE_PROTOCOL.as_bytes()
        {
            return Some(X11Cookie {
                protocol: COOKIE_PROTOCOL.into(),
                data,
            });
        }
    }
    None
}

/// Why a forwarded connection was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SetupError {
    /// Not an X11 setup request.
    Malformed,
    /// The request did not carry the fake cookie given to the server.
    WrongCookie,
}

/// The length of the X11 setup request at the start of `bytes`, once enough
/// of it has arrived to tell.
fn setup_len(bytes: &[u8]) -> Result<Option<usize>, SetupError> {
    let Some(header) = bytes.first_chunk::<12>() else {
        return Ok(None);
    };
    let read = |at: usize| match header[0] {
        b'B' => Ok(u16::from_be_bytes([header[at], header[at + 1]])),
        b'l' => Ok(u16::from_le_bytes([header[at], header[at + 1]])),
        _ => Err(SetupError::Malformed),
    };
    let name = usize::from(read(6)?);
    let data = usize::from(read(8)?);
    Ok(Some(12 + padded(name) + padded(data)))
}

fn padded(len: usize) -> usize {
    len.div_ceil(4) * 4
}

/// Check the cookie in a complete setup request and put in the real one
/// (or none, if the local display needs none).
pub fn rewrite_setup(
    setup: &[u8],
    fake: &[u8],
    real: Option<&X11Cookie>,
) -> Result<Vec<u8>, SetupError> {
    let header = setup.first_chunk::<12>().ok_or(SetupError::Malformed)?;
    let big_endian = match header[0] {
        b'B' => true,
        b'l' => false,
        _ => return Err(SetupError::Malformed),
    };
    let read = |at: usize| {
        let pair = [header[at], header[at + 1]];
        usize::from(if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        })
    };
    let (name_len, data_len) = (read(6), read(8));
    let name = setup.get(12..12 + name_len).ok_or(SetupError::Malformed)?;
    let data_at = 12 + padded(name_len);
    let data = setup
        .get(data_at..data_at + data_len)
        .ok_or(SetupError::Malformed)?;
    if name != COOKIE_PROTOCOL.as_bytes() || data != fake {
        return Err(SetupError::WrongCookie);
    }
    let (name, data) = match real {
        Some(cookie) => (cookie.protocol.as_bytes(), cookie.data.as_slice()),
        None => (&[][..], &[][..]),
    };
    let write = |value: usize| {
        let value = u16::try_from(value).unwrap_or(u16::MAX);
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let mut out = header[..6].to_vec();
    out.extend(write(name.len()));
    out.extend(write(data.len()));
    out.extend([0, 0]);
    for part in [name, data] {
        out.extend_from_slice(part);
        out.resize(out.len() + padded(part.len()) - part.len(), 0);
    }
    Ok(out)
}

/// Serve one forwarded connection: check and swap the cookie, then relay
/// bytes between the server's channel and the local X server.
pub async fn proxy<S>(
    mut channel: S,
    forwarding: &X11Forwarding,
    fake: &[u8],
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let invalid =
        |e: SetupError| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}"));
    let mut buffer = Vec::new();
    let len = loop {
        if let Some(len) = setup_len(&buffer).map_err(invalid)?
            && buffer.len() >= len
        {
            break len;
        }
        let mut chunk = [0u8; 1024];
        let read = channel.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let setup = rewrite_setup(&buffer[..len], fake, forwarding.cookie.as_ref()).map_err(invalid)?;
    match &forwarding.target {
        #[cfg(unix)]
        X11Target::Unix(path) => {
            let mut local = tokio::net::UnixStream::connect(path).await?;
            relay(&mut channel, &mut local, &setup, &buffer[len..]).await
        }
        #[cfg(not(unix))]
        X11Target::Unix(path) => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("{}: Unix sockets are not available here", path.display()),
        )),
        X11Target::Tcp { host, port } => {
            let mut local = tokio::net::TcpStream::connect((host.as_str(), *port)).await?;
            relay(&mut channel, &mut local, &setup, &buffer[len..]).await
        }
    }
}

async fn relay<A, B>(
    channel: &mut A,
    local: &mut B,
    setup: &[u8],
    rest: &[u8],
) -> std::io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    local.write_all(setup).await?;
    local.write_all(rest).await?;
    tokio::io::copy_bidirectional(channel, local).await?;
    Ok(())
}

/// Hex text of `bytes`, as cookies are sent to the server.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays() {
        let tcp = |host: &str, port| X11Target::Tcp {
            host: host.into(),
            port,
        };
        let d = parse_display("localhost:10.0").unwrap();
        assert_eq!(
            (d.target, d.number, d.screen),
            (tcp("localhost", 6010), 10, 0)
        );
        let d = parse_display("example.com:1.2").unwrap();
        assert_eq!(
            (d.target, d.number, d.screen),
            (tcp("example.com", 6001), 1, 2)
        );
        let d = parse_display(":0").unwrap();
        if cfg!(unix) {
            assert_eq!(d.target, X11Target::Unix("/tmp/.X11-unix/X0".into()));
        } else {
            assert_eq!(d.target, tcp("localhost", 6000));
        }
        assert!(parse_display("nonsense").is_none());
        assert!(parse_display(":x").is_none());
    }

    fn xauth_entry(family: u16, address: &[u8], number: &str, name: &str, data: &[u8]) -> Vec<u8> {
        let mut out = family.to_be_bytes().to_vec();
        for field in [address, number.as_bytes(), name.as_bytes(), data] {
            out.extend((field.len() as u16).to_be_bytes());
            out.extend_from_slice(field);
        }
        out
    }

    #[test]
    fn xauthority_cookies() {
        let mut file = xauth_entry(256, b"box", "1", COOKIE_PROTOCOL, &[1; 16]);
        file.extend(xauth_entry(
            256,
            b"box",
            "0",
            "XDM-AUTHORIZATION-1",
            &[9; 8],
        ));
        file.extend(xauth_entry(256, b"box", "0", COOKIE_PROTOCOL, &[2; 16]));
        assert_eq!(find_cookie(&file, 0).unwrap().data, vec![2; 16]);
        assert_eq!(find_cookie(&file, 1).unwrap().data, vec![1; 16]);
        assert!(find_cookie(&file, 7).is_none());
        assert!(find_cookie(&file[..10], 0).is_none(), "truncated file");
    }

    fn setup(big_endian: bool, name: &[u8], data: &[u8]) -> Vec<u8> {
        let word = |v: u16| {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut out = vec![if big_endian { b'B' } else { b'l' }, 0];
        out.extend(word(11));
        out.extend(word(0));
        out.extend(word(name.len() as u16));
        out.extend(word(data.len() as u16));
        out.extend([0, 0]);
        for part in [name, data] {
            out.extend_from_slice(part);
            out.resize(out.len() + padded(part.len()) - part.len(), 0);
        }
        out
    }

    #[test]
    fn setup_cookie_is_swapped() {
        let fake = [7u8; 16];
        let real = X11Cookie {
            protocol: COOKIE_PROTOCOL.into(),
            data: vec![3; 16],
        };
        for big_endian in [true, false] {
            let request = setup(big_endian, COOKIE_PROTOCOL.as_bytes(), &fake);
            assert_eq!(setup_len(&request), Ok(Some(request.len())));
            assert_eq!(setup_len(&request[..11]), Ok(None));
            let out = rewrite_setup(&request, &fake, Some(&real)).unwrap();
            assert_eq!(
                out,
                setup(big_endian, COOKIE_PROTOCOL.as_bytes(), &real.data)
            );
            let out = rewrite_setup(&request, &fake, None).unwrap();
            assert_eq!(out, setup(big_endian, b"", b""));
            let wrong = setup(big_endian, COOKIE_PROTOCOL.as_bytes(), &[0; 16]);
            assert_eq!(
                rewrite_setup(&wrong, &fake, None),
                Err(SetupError::WrongCookie)
            );
        }
        assert_eq!(setup_len(b"x23456789012"), Err(SetupError::Malformed));
    }
}
