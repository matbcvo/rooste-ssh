//! Shared test fixtures.
//!
//! - [`TestServer`]: in-process SSH server with a fake shell, runs everywhere.
//! - [`TestAgent`]: in-process SSH agent holding the `id_ed25519` test key.
//! - [`OpenSshFixture`]: a real OpenSSH server. Tests using it are marked
//!   `#[ignore]` and run in CI with `cargo nextest run --run-ignored only`.
//!   Start it locally with:
//!
//! ```text
//! docker build -t rooste-openssh -f crates/rooste-testkit/docker/openssh/Dockerfile crates/rooste-testkit/docker
//! docker run -d --rm -p 2222:22 --name rooste-openssh rooste-openssh
//! ```

mod agent;
mod echo;
mod server;
mod sftp;
mod telnet;

use std::io::{self, BufRead, BufReader};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use russh::keys::{PrivateKey, PublicKey};

pub use agent::TestAgent;
pub use echo::EchoServer;
pub use server::TestServer;
pub use telnet::TelnetServer;

/// Credentials accepted by [`TestServer`] and the Docker fixture. Test-only.
pub const TEST_USER: &str = "rooste";
pub const TEST_PASSWORD: &str = "rooste";
/// Passphrase of the `id_ed25519_encrypted` test key.
pub const TEST_KEY_PASSPHRASE: &str = "rooste";

/// Path of a key in the `keys/` fixture directory.
pub fn key_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("keys")
        .join(name)
}

/// Load an unencrypted private test key.
pub fn private_key(name: &str) -> PrivateKey {
    russh::keys::load_secret_key(key_path(name), None)
        .unwrap_or_else(|e| panic!("load test key {name}: {e}"))
}

/// Load the public half of a test key.
pub fn public_key(name: &str) -> PublicKey {
    russh::keys::load_public_key(key_path(&format!("{name}.pub")))
        .unwrap_or_else(|e| panic!("load test public key {name}: {e}"))
}

/// Environment variables overriding the OpenSSH fixture.
pub const OPENSSH_ADDR_ENV: &str = "ROOSTE_TEST_SSH_ADDR";
pub const OPENSSH_USER_ENV: &str = "ROOSTE_TEST_SSH_USER";
pub const OPENSSH_PASSWORD_ENV: &str = "ROOSTE_TEST_SSH_PASSWORD";
const DEFAULT_OPENSSH_ADDR: &str = "127.0.0.1:2222";

/// A real OpenSSH server: the Docker fixture on Linux, the Windows OpenSSH
/// Server in Windows CI. All client test keys are authorized for `user`.
#[derive(Debug, Clone)]
pub struct OpenSshFixture {
    pub addr: SocketAddr,
    pub user: String,
    pub password: String,
}

impl OpenSshFixture {
    /// Read the fixture from the `ROOSTE_TEST_SSH_*` variables, defaulting
    /// to the Docker fixture on `127.0.0.1:2222`.
    pub fn from_env() -> io::Result<Self> {
        let raw = std::env::var(OPENSSH_ADDR_ENV).unwrap_or_else(|_| DEFAULT_OPENSSH_ADDR.into());
        let addr = raw.to_socket_addrs()?.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no address for `{raw}`"),
            )
        })?;
        Ok(Self {
            addr,
            user: std::env::var(OPENSSH_USER_ENV).unwrap_or_else(|_| TEST_USER.into()),
            password: std::env::var(OPENSSH_PASSWORD_ENV).unwrap_or_else(|_| TEST_PASSWORD.into()),
        })
    }

    /// Connect and return the server's SSH identification line.
    pub fn read_banner(&self) -> io::Result<String> {
        let stream = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line.trim_end().to_owned())
    }
}
