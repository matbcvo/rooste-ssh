//! An in-process telnet server for tests. On connect it asks for the window
//! size (NAWS) and terminal type (TTYPE) and offers to echo, like a real
//! server. Lines: `size` prints `COLSxROWS`, `ttype` the terminal type, and
//! anything else is echoed as `echo: <line>`; each reply ends with `$ `.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const IAC: u8 = 255;
const DO: u8 = 253;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const ECHO: u8 = 1;
const SGA: u8 = 3;
const TTYPE: u8 = 24;
const NAWS: u8 = 31;

/// A running telnet server. Stops when dropped.
#[derive(Debug)]
pub struct TelnetServer {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl TelnetServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind telnet");
        let addr = listener.local_addr().expect("telnet address");
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket));
            }
        });
        Self { addr, task }
    }
}

impl Drop for TelnetServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct Client {
    size: (u16, u16),
    term: String,
    line: Vec<u8>,
}

async fn serve(mut socket: TcpStream) {
    let hello = [
        IAC, DO, NAWS, IAC, DO, TTYPE, IAC, WILL, ECHO, IAC, WILL, SGA, IAC, SB, TTYPE, 1, IAC, SE,
    ];
    if socket.write_all(&hello).await.is_err() {
        return;
    }
    let _ = socket.write_all(b"Welcome\r\n$ ").await;
    let mut client = Client::default();
    let mut buf = [0u8; 1024];
    let mut input: Vec<u8> = Vec::new();
    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        input.extend_from_slice(&buf[..n]);
        let mut out = Vec::new();
        let consumed = process(&mut client, &input, &mut out);
        input.drain(..consumed);
        if !out.is_empty() && socket.write_all(&out).await.is_err() {
            return;
        }
    }
}

/// Handle complete commands and data in `input`; returns bytes consumed.
fn process(client: &mut Client, input: &[u8], out: &mut Vec<u8>) -> usize {
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            IAC => {
                let Some(&command) = input.get(i + 1) else {
                    return i;
                };
                match command {
                    IAC => {
                        client.line.push(IAC);
                        out.extend([IAC, IAC]);
                        i += 2;
                    }
                    SB => {
                        let Some(end) = input[i..].windows(2).position(|w| w == [IAC, SE]) else {
                            return i;
                        };
                        let body = &input[i + 2..i + end];
                        match body.first() {
                            Some(&NAWS) if body.len() >= 5 => {
                                client.size = (
                                    u16::from_be_bytes([body[1], body[2]]),
                                    u16::from_be_bytes([body[3], body[4]]),
                                );
                            }
                            Some(&TTYPE) if body.get(1) == Some(&0) => {
                                client.term = String::from_utf8_lossy(&body[2..]).into_owned();
                            }
                            _ => {}
                        }
                        i += end + 2;
                    }
                    _ => {
                        if input.len() < i + 3 {
                            return i;
                        }
                        i += 3;
                    }
                }
            }
            b'\r' | b'\n' => {
                // CR NUL or CR LF ends a line once.
                let skip = matches!(input.get(i + 1), Some(0) | Some(b'\n'));
                let line = String::from_utf8_lossy(&std::mem::take(&mut client.line)).into_owned();
                out.extend_from_slice(b"\r\n");
                let reply = match line.trim() {
                    "" => String::new(),
                    "size" => format!("{}x{}\r\n", client.size.0, client.size.1),
                    "ttype" => format!("{}\r\n", client.term),
                    other => format!("echo: {other}\r\n"),
                };
                out.extend_from_slice(reply.as_bytes());
                out.extend_from_slice(b"$ ");
                i += if skip { 2 } else { 1 };
            }
            byte => {
                client.line.push(byte);
                out.push(byte);
                i += 1;
            }
        }
    }
    i
}
