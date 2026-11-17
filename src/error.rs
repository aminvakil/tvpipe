//! Crate-wide error type.
//!
//! Conventions (frozen in M0, see `docs/decisions/0001-core-types.md`):
//!
//! - Every fallible function returns [`Result<T>`].
//! - Data from the network or from ffmpeg never causes a panic; malformed
//!   input becomes [`ErrorKind::Malformed`] with a short message that names
//!   the format, e.g. `"bplist: offset table out of range"`.
//! - Modules may keep private error enums internally, but convert to
//!   [`Error`] at their public boundary.

use std::borrow::Cow;
use std::fmt;

/// What went wrong, coarsely. Used for exit codes and retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// An I/O operation failed (sockets, files, pipes).
    Io,
    /// Input bytes do not parse (any format: DNS, bplist, mkv, RTP, ...).
    Malformed,
    /// The peer sent something well-formed but unexpected for the protocol
    /// state, or returned an error status.
    Protocol,
    /// A cryptographic operation failed (bad tag, bad signature, SRP proof).
    Crypto,
    /// Authentication or pairing was refused or is required.
    Auth,
    /// The receiver or the local ffmpeg does not support what we need.
    Unsupported,
    /// A deadline passed.
    Timeout,
    /// A device, file, or credential was not found.
    NotFound,
    /// The peer or pipe closed the connection.
    Closed,
    /// An external process (ffmpeg) failed.
    Process,
    /// Invalid user input that the CLI parser could not catch.
    Usage,
    /// Not implemented yet.
    Unimplemented,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Io => "I/O error",
            ErrorKind::Malformed => "malformed data",
            ErrorKind::Protocol => "protocol error",
            ErrorKind::Crypto => "crypto error",
            ErrorKind::Auth => "authentication error",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Timeout => "timed out",
            ErrorKind::NotFound => "not found",
            ErrorKind::Closed => "connection closed",
            ErrorKind::Process => "process error",
            ErrorKind::Usage => "usage error",
            ErrorKind::Unimplemented => "not implemented",
        }
    }
}

/// The crate-wide error.
pub struct Error {
    kind: ErrorKind,
    msg: Cow<'static, str>,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(kind: ErrorKind, msg: impl Into<Cow<'static, str>>) -> Error {
        Error {
            kind,
            msg: msg.into(),
            source: None,
        }
    }

    /// Attaches an underlying cause.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Error {
        self.source = Some(Box::new(source));
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.msg
    }

    pub fn malformed(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Malformed, msg)
    }

    pub fn protocol(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Protocol, msg)
    }

    pub fn crypto(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Crypto, msg)
    }

    pub fn auth(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Auth, msg)
    }

    pub fn unsupported(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Unsupported, msg)
    }

    pub fn timeout(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Timeout, msg)
    }

    pub fn not_found(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::NotFound, msg)
    }

    pub fn closed(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Closed, msg)
    }

    pub fn process(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Process, msg)
    }

    pub fn usage(msg: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Usage, msg)
    }

    pub fn unimplemented(what: impl Into<Cow<'static, str>>) -> Error {
        Error::new(ErrorKind::Unimplemented, what)
    }

    /// Wraps an I/O error with context, e.g. `Error::io("connect to TV", e)`.
    pub fn io(context: impl Into<Cow<'static, str>>, err: std::io::Error) -> Error {
        let kind = match err.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ErrorKind::Timeout,
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe => ErrorKind::Closed,
            std::io::ErrorKind::NotFound => ErrorKind::NotFound,
            _ => ErrorKind::Io,
        };
        Error::new(kind, context).with_source(err)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Error");
        s.field("kind", &self.kind).field("msg", &self.msg);
        if let Some(ref source) = self.source {
            s.field("source", source);
        }
        s.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.msg.is_empty() {
            f.write_str(self.kind.as_str())?;
        } else {
            f.write_str(&self.msg)?;
        }
        if let Some(ref source) = self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|s| &**s as &(dyn std::error::Error + 'static))
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::io("", err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_source() {
        let err = Error::io(
            "connect",
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset"),
        );
        assert_eq!(err.kind(), ErrorKind::Closed);
        assert_eq!(err.to_string(), "connect: reset");
    }
}
