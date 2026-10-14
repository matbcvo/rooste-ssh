//! Shared test fixtures.
//!
//! Tests that need a real OpenSSH server are marked `#[ignore]` and run in CI
//! with `cargo nextest run --run-ignored only`. Start the server locally with:
//!
//! ```text
//! docker build -t rooste-openssh crates/rooste-testkit/docker/openssh
//! docker run -d --rm -p 2222:22 --name rooste-openssh rooste-openssh
//! ```

use std::io::{self, BufRead, BufReader};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Environment variable overriding the OpenSSH fixture address.
pub const OPENSSH_ADDR_ENV: &str = "ROOSTE_TEST_SSH_ADDR";
const DEFAULT_OPENSSH_ADDR: &str = "127.0.0.1:2222";

/// Credentials baked into the Docker fixture. Test-only, never use elsewhere.
pub const TEST_USER: &str = "rooste";
pub const TEST_PASSWORD: &str = "rooste";

/// The OpenSSH server started from `docker/openssh`.
#[derive(Debug, Clone)]
pub struct OpenSshFixture {
    pub addr: SocketAddr,
}

impl OpenSshFixture {
    /// Locate the fixture from [`OPENSSH_ADDR_ENV`], defaulting to `127.0.0.1:2222`.
    pub fn from_env() -> io::Result<Self> {
        let raw = std::env::var(OPENSSH_ADDR_ENV).unwrap_or_else(|_| DEFAULT_OPENSSH_ADDR.into());
        let addr = raw.to_socket_addrs()?.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no address for `{raw}`"),
            )
        })?;
        Ok(Self { addr })
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
