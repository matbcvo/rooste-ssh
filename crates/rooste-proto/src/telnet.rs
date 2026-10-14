//! Telnet (RFC 854) with the options a terminal needs: the server may echo
//! and suppress go-ahead, and we report the window size (NAWS, RFC 1073)
//! and terminal type (TTYPE, RFC 1091). Everything else is refused.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::Transport;
use crate::raw::connect_tcp;

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const ECHO: u8 = 1;
const SGA: u8 = 3;
const TTYPE: u8 = 24;
const NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    /// After IAC WILL/WONT/DO/DONT, waiting for the option byte.
    Command(u8),
    Sub,
    SubIac,
}

/// Telnet protocol state, independent of the socket.
#[derive(Debug, Clone)]
pub struct Telnet {
    state: State,
    sub: Vec<u8>,
    /// Options the server has enabled on its side.
    server: [bool; 256],
    /// Options enabled on our side.
    client: [bool; 256],
    term: String,
    size: (u16, u16),
    /// The last input byte was CR (for CR NUL translation).
    pending_cr: bool,
}

impl Telnet {
    pub fn new(term: &str, cols: u16, rows: u16) -> Self {
        Self {
            state: State::Data,
            sub: Vec::new(),
            server: [false; 256],
            client: [false; 256],
            term: term.to_owned(),
            size: (cols, rows),
            pending_cr: false,
        }
    }

    /// Whether the server echoes what we type.
    pub fn server_echoes(&self) -> bool {
        self.server[usize::from(ECHO)]
    }

    pub fn naws_enabled(&self) -> bool {
        self.client[usize::from(NAWS)]
    }

    /// Process bytes from the server: returns terminal data and the bytes
    /// to send back (option replies).
    pub fn receive(&mut self, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::with_capacity(input.len());
        let mut reply = Vec::new();
        for &byte in input {
            self.state = match (self.state, byte) {
                (State::Data, IAC) => State::Iac,
                (State::Data, b) => {
                    data.push(b);
                    State::Data
                }
                (State::Iac, IAC) => {
                    data.push(IAC);
                    State::Data
                }
                (State::Iac, WILL | WONT | DO | DONT) => State::Command(byte),
                (State::Iac, SB) => {
                    self.sub.clear();
                    State::Sub
                }
                // NOP, GA, etc.
                (State::Iac, _) => State::Data,
                (State::Command(command), option) => {
                    self.negotiate(command, option, &mut reply);
                    State::Data
                }
                (State::Sub, IAC) => State::SubIac,
                (State::Sub, b) => {
                    if self.sub.len() < 1024 {
                        self.sub.push(b);
                    }
                    State::Sub
                }
                (State::SubIac, SE) => {
                    self.subnegotiation(&mut reply);
                    State::Data
                }
                (State::SubIac, IAC) => {
                    self.sub.push(IAC);
                    State::Sub
                }
                (State::SubIac, _) => State::Sub,
            };
        }
        (data, reply)
    }

    fn negotiate(&mut self, command: u8, option: u8, reply: &mut Vec<u8>) {
        let o = usize::from(option);
        match command {
            WILL => {
                let accept = matches!(option, ECHO | SGA);
                if accept && !self.server[o] {
                    self.server[o] = true;
                    reply.extend([IAC, DO, option]);
                } else if !accept {
                    reply.extend([IAC, DONT, option]);
                }
            }
            WONT if self.server[o] => {
                self.server[o] = false;
                reply.extend([IAC, DONT, option]);
            }
            DO => {
                let accept = matches!(option, NAWS | TTYPE);
                if accept && !self.client[o] {
                    self.client[o] = true;
                    reply.extend([IAC, WILL, option]);
                    if option == NAWS {
                        reply.extend(self.naws());
                    }
                } else if !accept {
                    reply.extend([IAC, WONT, option]);
                }
            }
            DONT if self.client[o] => {
                self.client[o] = false;
                reply.extend([IAC, WONT, option]);
            }
            _ => {}
        }
    }

    fn subnegotiation(&mut self, reply: &mut Vec<u8>) {
        if self.sub.as_slice() == [TTYPE, TTYPE_SEND] && self.client[usize::from(TTYPE)] {
            reply.extend([IAC, SB, TTYPE, TTYPE_IS]);
            reply.extend(self.term.bytes());
            reply.extend([IAC, SE]);
        }
    }

    /// The NAWS subnegotiation for the current size.
    fn naws(&self) -> Vec<u8> {
        let mut out = vec![IAC, SB, NAWS];
        for byte in self
            .size
            .0
            .to_be_bytes()
            .into_iter()
            .chain(self.size.1.to_be_bytes())
        {
            out.push(byte);
            if byte == IAC {
                out.push(IAC);
            }
        }
        out.extend([IAC, SE]);
        out
    }

    /// Record a new window size; returns the update to send, if the server
    /// asked for window sizes.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.size = (cols, rows);
        self.naws_enabled().then(|| self.naws())
    }

    /// Encode terminal input: IAC is doubled and a bare CR becomes CR NUL.
    pub fn encode(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() + 4);
        for &byte in input {
            if self.pending_cr && byte != b'\n' {
                out.push(0);
            }
            self.pending_cr = byte == b'\r';
            out.push(byte);
            if byte == IAC {
                out.push(IAC);
            }
        }
        if self.pending_cr {
            // Enter arrives as a lone CR; don't wait for the next key.
            out.push(0);
            self.pending_cr = false;
        }
        out
    }
}

/// A telnet session.
#[derive(Debug)]
pub struct TelnetTransport {
    stream: TcpStream,
    telnet: Telnet,
    pending: Vec<u8>,
    pos: usize,
}

impl TelnetTransport {
    pub async fn connect(
        host: &str,
        port: u16,
        timeout: Duration,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> io::Result<Self> {
        Ok(Self {
            stream: connect_tcp(host, port, timeout).await?,
            telnet: Telnet::new(term, cols, rows),
            pending: Vec::new(),
            pos: 0,
        })
    }

    pub fn state(&self) -> &Telnet {
        &self.telnet
    }
}

impl Transport for TelnetTransport {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut raw = vec![0u8; buf.len().max(512)];
        loop {
            if self.pos < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.pos);
                buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            let n = self.stream.read(&mut raw).await?;
            if n == 0 {
                return Ok(0);
            }
            let (data, reply) = self.telnet.receive(&raw[..n]);
            if !reply.is_empty() {
                self.stream.write_all(&reply).await?;
            }
            self.pending = data;
            self.pos = 0;
        }
    }

    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let encoded = self.telnet.encode(data);
        self.stream.write_all(&encoded).await
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        match self.telnet.resize(cols, rows) {
            Some(update) => self.stream.write_all(&update).await,
            None => Ok(()),
        }
    }

    async fn close(&mut self) -> io::Result<()> {
        self.stream.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telnet() -> Telnet {
        Telnet::new("xterm-256color", 80, 24)
    }

    #[test]
    fn plain_data_and_escaped_iac() {
        let mut t = telnet();
        let (data, reply) = t.receive(b"hello \xff\xffworld");
        assert_eq!(data, b"hello \xffworld");
        assert!(reply.is_empty());
    }

    #[test]
    fn accepts_echo_and_sga_refuses_others() {
        let mut t = telnet();
        let (data, reply) = t.receive(&[IAC, WILL, ECHO, IAC, WILL, SGA, IAC, WILL, 34]);
        assert!(data.is_empty());
        assert_eq!(reply, [IAC, DO, ECHO, IAC, DO, SGA, IAC, DONT, 34]);
        assert!(t.server_echoes());
        // Repeated WILL is not answered again (no negotiation loops).
        assert!(t.receive(&[IAC, WILL, ECHO]).1.is_empty());
        assert_eq!(t.receive(&[IAC, WONT, ECHO]).1, [IAC, DONT, ECHO]);
        assert!(!t.server_echoes());
    }

    #[test]
    fn reports_window_size_and_updates() {
        let mut t = telnet();
        assert_eq!(t.resize(100, 30), None, "not until the server asks");
        let (_, reply) = t.receive(&[IAC, DO, NAWS]);
        assert_eq!(
            reply,
            [IAC, WILL, NAWS, IAC, SB, NAWS, 0, 100, 0, 30, IAC, SE]
        );
        assert_eq!(
            t.resize(255, 1).unwrap(),
            [IAC, SB, NAWS, 0, 255, 255, 0, 1, IAC, SE],
            "a 255 byte is doubled"
        );
        assert_eq!(t.receive(&[IAC, DONT, NAWS]).1, [IAC, WONT, NAWS]);
        assert_eq!(t.resize(80, 24), None);
    }

    #[test]
    fn sends_terminal_type_when_asked() {
        let mut t = telnet();
        assert_eq!(t.receive(&[IAC, DO, TTYPE]).1, [IAC, WILL, TTYPE]);
        let (_, reply) = t.receive(&[IAC, SB, TTYPE, TTYPE_SEND, IAC, SE]);
        let mut expected = vec![IAC, SB, TTYPE, TTYPE_IS];
        expected.extend(b"xterm-256color");
        expected.extend([IAC, SE]);
        assert_eq!(reply, expected);
    }

    #[test]
    fn refuses_unknown_options_we_are_asked_for() {
        let mut t = telnet();
        assert_eq!(t.receive(&[IAC, DO, 39]).1, [IAC, WONT, 39]);
    }

    #[test]
    fn negotiation_split_across_reads() {
        let mut t = telnet();
        let mut reply = Vec::new();
        for byte in [b'a', IAC, DO, NAWS, b'b'] {
            let (_, r) = t.receive(&[byte]);
            reply.extend(r);
        }
        assert!(reply.starts_with(&[IAC, WILL, NAWS]));
    }

    #[test]
    fn input_encoding() {
        let mut t = telnet();
        assert_eq!(t.encode(b"ls\r"), b"ls\r\0");
        assert_eq!(t.encode(b"a\r\nb"), b"a\r\nb");
        assert_eq!(t.encode(&[IAC]), [IAC, IAC]);
    }
}
