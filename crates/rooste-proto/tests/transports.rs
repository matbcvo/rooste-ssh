//! The same conformance checks for every transport: data both ways,
//! resize, and orderly close. Raw TCP and telnet run everywhere; serial uses
//! a pseudo-terminal pair on Unix.

use std::time::Duration;

use rooste_proto::Transport;
use rooste_proto::raw::RawTransport;
use rooste_proto::telnet::TelnetTransport;
use rooste_testkit::{EchoServer, TelnetServer};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Read until `needle` has been seen.
async fn read_until<T: Transport>(t: &mut T, needle: &str) -> String {
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    tokio::time::timeout(TIMEOUT, async {
        while !String::from_utf8_lossy(&seen).contains(needle) {
            let n = t.read(&mut buf).await.expect("read");
            assert!(
                n > 0,
                "closed early; got {:?}",
                String::from_utf8_lossy(&seen)
            );
            seen.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "did not see {needle:?}; got {:?}",
            String::from_utf8_lossy(&seen)
        )
    });
    String::from_utf8_lossy(&seen).into_owned()
}

/// Shared checks: `send` must come back containing `expect`.
async fn conformance<T: Transport>(t: &mut T, send: &[u8], expect: &str) {
    t.write(send).await.expect("write");
    read_until(t, expect).await;
    t.resize(120, 40).await.expect("resize");
    // Data containing every byte value survives the trip where the
    // transport is transparent (checked separately per transport).
    t.close().await.expect("close");
}

#[tokio::test]
async fn raw_tcp() {
    let echo = EchoServer::start().await;
    let mut t = RawTransport::connect("127.0.0.1", echo.addr.port(), TIMEOUT)
        .await
        .unwrap();
    // Raw is binary-transparent.
    let all: Vec<u8> = (0..=255u8).collect();
    t.write(&all).await.unwrap();
    let mut back = Vec::new();
    let mut buf = [0u8; 512];
    while back.len() < all.len() {
        let n = t.read(&mut buf).await.unwrap();
        back.extend_from_slice(&buf[..n]);
    }
    assert_eq!(back, all);
    conformance(&mut t, b"ping", "ping").await;
}

#[tokio::test]
async fn raw_tcp_connection_refused() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    assert!(
        RawTransport::connect("127.0.0.1", port, TIMEOUT)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn telnet_negotiation_size_and_terminal_type() {
    let server = TelnetServer::start().await;
    let mut t = TelnetTransport::connect(
        "127.0.0.1",
        server.addr.port(),
        TIMEOUT,
        "xterm-256color",
        80,
        24,
    )
    .await
    .unwrap();
    read_until(&mut t, "$ ").await;
    assert!(t.state().server_echoes());
    assert!(t.state().naws_enabled());

    t.write(b"size\r").await.unwrap();
    read_until(&mut t, "80x24").await;
    t.write(b"ttype\r").await.unwrap();
    read_until(&mut t, "xterm-256color").await;

    t.resize(132, 43).await.unwrap();
    t.write(b"size\r").await.unwrap();
    read_until(&mut t, "132x43").await;

    // A 0xFF data byte is escaped on the way out and reaches the server as
    // one byte (shown as U+FFFD in its UTF-8 reply).
    t.write(b"a\xffb\r").await.unwrap();
    read_until(&mut t, "echo: a\u{fffd}b").await;

    conformance(&mut t, b"hello\r", "echo: hello").await;
}

#[cfg(unix)]
#[tokio::test]
async fn serial_pseudo_terminal_pair() {
    use rooste_proto::serial::SerialTransport;
    use serial2_tokio::SerialPort;

    let (ours, theirs) = SerialPort::pair().expect("pty pair");
    // The far end echoes, like a device in loopback.
    let echo = tokio::spawn(async move {
        let mut buf = [0u8; 256];
        while let Ok(n) = theirs.read(&mut buf).await {
            if n == 0 || theirs.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
    });
    let mut t = SerialTransport::from_port(ours);
    conformance(&mut t, b"AT\r", "AT").await;
    echo.abort();
}

#[test]
fn serial_port_listing_does_not_fail() {
    // Machines may have no ports; listing must still work.
    let _ = rooste_proto::serial::available_ports();
}

#[test]
fn opening_a_missing_serial_port_names_it() {
    let missing = if cfg!(windows) {
        "COM250"
    } else {
        "/dev/rooste-no-such-port"
    };
    let err = rooste_proto::serial::SerialTransport::open(missing, Default::default()).unwrap_err();
    assert!(err.to_string().contains(missing), "{err}");
}

/// BusyBox telnetd running a shell (Docker in Linux CI).
#[tokio::test]
#[ignore = "requires the telnet server container"]
async fn busybox_telnetd() {
    let addr = std::env::var("ROOSTE_TEST_TELNET_ADDR").expect("set ROOSTE_TEST_TELNET_ADDR");
    let (host, port) = addr.rsplit_once(':').unwrap();
    let mut t = TelnetTransport::connect(host, port.parse().unwrap(), TIMEOUT, "xterm", 120, 40)
        .await
        .unwrap();
    t.write(b"echo $((6*7))\r").await.unwrap();
    read_until(&mut t, "42").await;
    t.write(b"stty size\r").await.unwrap();
    read_until(&mut t, "40 120").await;
    t.resize(100, 30).await.unwrap();
    t.write(b"stty size\r").await.unwrap();
    read_until(&mut t, "30 100").await;
}
