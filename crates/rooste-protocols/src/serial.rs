//! Serial ports: `COMx` on Windows, `/dev/ttyUSB0` and similar on Linux.

use std::io;

use serial2_tokio::{CharSize, FlowControl, Parity, SerialPort, Settings, StopBits};

use crate::Transport;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SerialParity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SerialFlow {
    #[default]
    None,
    XonXoff,
    RtsCts,
}

/// Line settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialOptions {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: SerialParity,
    pub stop_bits: u8,
    pub flow: SerialFlow,
}

impl Default for SerialOptions {
    fn default() -> Self {
        Self {
            baud: 9600,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: 1,
            flow: SerialFlow::None,
        }
    }
}

impl SerialOptions {
    fn apply(self, mut settings: Settings) -> io::Result<Settings> {
        settings.set_raw();
        settings.set_baud_rate(self.baud)?;
        settings.set_char_size(match self.data_bits {
            5 => CharSize::Bits5,
            6 => CharSize::Bits6,
            7 => CharSize::Bits7,
            _ => CharSize::Bits8,
        });
        settings.set_stop_bits(if self.stop_bits == 2 {
            StopBits::Two
        } else {
            StopBits::One
        });
        settings.set_parity(match self.parity {
            SerialParity::None => Parity::None,
            SerialParity::Odd => Parity::Odd,
            SerialParity::Even => Parity::Even,
        });
        settings.set_flow_control(match self.flow {
            SerialFlow::None => FlowControl::None,
            SerialFlow::XonXoff => FlowControl::XonXoff,
            SerialFlow::RtsCts => FlowControl::RtsCts,
        });
        Ok(settings)
    }
}

/// Serial ports present on this machine.
pub fn available_ports() -> Vec<String> {
    SerialPort::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.display().to_string()).collect())
        .unwrap_or_default()
}

/// An open serial port.
#[derive(Debug)]
pub struct SerialTransport {
    port: SerialPort,
}

impl SerialTransport {
    pub fn open(path: &str, options: SerialOptions) -> io::Result<Self> {
        let port = SerialPort::open(path, move |settings| options.apply(settings))
            .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))?;
        Ok(Self { port })
    }

    /// Wrap an already open port (used by tests with pseudo-terminal pairs).
    pub fn from_port(port: SerialPort) -> Self {
        Self { port }
    }
}

impl Transport for SerialTransport {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.port.read(buf).await
    }

    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.port.write_all(data).await
    }

    async fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}
