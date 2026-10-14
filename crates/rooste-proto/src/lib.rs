//! Transport abstraction.
//!
//! The terminal widget, tabs, logging and input broadcast only talk to
//! [`Transport`], so new protocols plug in without touching the UI.

use std::future::Future;
use std::io;

pub mod raw;
pub mod serial;
pub mod ssh;
pub mod telnet;

/// A bidirectional byte stream backing one terminal session.
pub trait Transport: Send {
    /// Read bytes from the remote side. Returns `Ok(0)` on end of stream.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;

    /// Write all of `data` to the remote side.
    fn write(&mut self, data: &[u8]) -> impl Future<Output = io::Result<()>> + Send;

    /// Tell the remote side the terminal size changed. Transports without a
    /// notion of window size keep this default no-op.
    fn resize(&mut self, _cols: u16, _rows: u16) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }

    /// Close the connection.
    fn close(&mut self) -> impl Future<Output = io::Result<()>> + Send;
}
