use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoggerError {
    #[error("logger connection failed: {0}")]
    Io(io::Error),
    #[error("logger did not answer in time")]
    Timeout,
    #[error("logger closed the connection")]
    Closed,
    #[error("frame checksum mismatch: trailer {expected:#06x}, payload sums to {actual:#06x}")]
    Checksum { expected: u16, actual: u16 },
    #[error("unexpected reply from logger: {0}")]
    Protocol(String),
    #[error("logger reported status {0:#06x}")]
    Status(u32),
    #[error("{0} is not on the logger")]
    NotFound(String),
    #[error("transfer incomplete: {received} of {expected} bytes")]
    SizeMismatch { expected: u64, received: u64 },
    #[error("cancelled")]
    Cancelled,
}

impl From<io::Error> for LoggerError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Self::Timeout,
            io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe => Self::Closed,
            _ => Self::Io(error),
        }
    }
}

impl LoggerError {
    /// The logger could not be reached at all, as opposed to answering badly.
    /// Callers polling for a logger treat these as "not in range".
    pub fn is_unreachable(&self) -> bool {
        match self {
            Self::Timeout | Self::Closed => true,
            Self::Io(error) => matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::NetworkDown
                    | io::ErrorKind::AddrNotAvailable
            ),
            _ => false,
        }
    }
}

pub type Result<T, E = LoggerError> = std::result::Result<T, E>;
