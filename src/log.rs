//! Logging: off by default, a global handler the program may install.
//!
//! The library never writes to stdout or stderr. A program that wants
//! tvpipe's log lines installs a handler once with [`set_logger`]; until
//! then a log statement costs one relaxed atomic load and a branch (the
//! message is never formatted). The handler runs on whatever tvpipe
//! thread logs, so it must be quick. The CLI installs its own: stderr
//! lines, or `log` events with `--machine`.
//!
//! Inside the crate: the macros `error!`, `warn!`, `info!`, `debug!`,
//! `trace!`.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// How important a log line is; `Error` is the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

/// One log line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Record {
    pub level: Level,
    /// The module that logged it, without the crate name (`airplay::sink`).
    pub module: String,
    pub message: String,
}

impl Level {
    /// Parses a level's wire name (`as_str`).
    pub fn from_wire(s: &str) -> Option<Level> {
        [
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ]
        .into_iter()
        .find(|l| l.as_str() == s)
    }

    /// The level in `log` events (`--machine`).
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }

    /// A fixed-width upper-case label (`"WARN "`), for log lines.
    pub fn label(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Record {
    /// The record as a machine-interface `log` event line.
    pub fn to_json(&self) -> String {
        crate::events::Event::Log(self.clone()).to_json()
    }
}

/// The handler [`set_logger`] installs.
pub type Logger = dyn Fn(&Record) + Send + Sync;

/// The most verbose level that reaches the handler; 0 (off) until one is
/// installed.
static LEVEL: AtomicU8 = AtomicU8::new(0);
static LOGGER: OnceLock<Box<Logger>> = OnceLock::new();

/// [`set_logger`] was called before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetLoggerError;

impl std::fmt::Display for SetLoggerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a tvpipe logger is already installed")
    }
}

impl std::error::Error for SetLoggerError {}

/// Installs the process's log handler, receiving lines at `level` and
/// above. Once per process (a second call is an error).
pub fn set_logger(
    level: Level,
    f: impl Fn(&Record) + Send + Sync + 'static,
) -> std::result::Result<(), SetLoggerError> {
    LOGGER.set(Box::new(f)).map_err(|_| SetLoggerError)?;
    LEVEL.store(level as u8, Ordering::Relaxed);
    Ok(())
}

/// Changes the level of the installed handler (nothing without one).
pub fn set_level(level: Level) {
    if LOGGER.get().is_some() {
        LEVEL.store(level as u8, Ordering::Relaxed);
    }
}

/// Whether a line at `level` would reach a handler.
#[inline]
pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

/// Tests that want to see the log: stderr at `level` (the first call
/// installs it).
#[cfg(test)]
pub fn to_stderr(level: Level) {
    let _ = set_logger(level, |r| {
        eprintln!("{} {}: {}", r.level.label(), r.module, r.message)
    });
    set_level(level);
}

#[doc(hidden)]
pub fn write(level: Level, module: &str, args: std::fmt::Arguments<'_>) {
    let Some(logger) = LOGGER.get() else { return };
    let module = module.strip_prefix("tvpipe::").unwrap_or(module);
    // Messages quote peers (receiver names, pages over a tunnel): no
    // terminal escape sequences get through to whoever prints them.
    let mut message = args.to_string();
    let bad = |c: char| c.is_control() && c != '\n' && c != '\t';
    if message.chars().any(bad) {
        message = message
            .chars()
            .map(|c| if bad(c) { '?' } else { c })
            .collect();
    }
    logger(&Record {
        level,
        module: module.to_owned(),
        message,
    });
}

#[macro_export]
#[doc(hidden)]
macro_rules! log_at {
    ($level:expr, $($arg:tt)+) => {
        if $crate::log::enabled($level) {
            $crate::log::write($level, module_path!(), format_args!($($arg)+));
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! error { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Error, $($arg)+) }; }
#[macro_export]
#[doc(hidden)]
macro_rules! warn { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Warn, $($arg)+) }; }
#[macro_export]
#[doc(hidden)]
macro_rules! info { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Info, $($arg)+) }; }
#[macro_export]
#[doc(hidden)]
macro_rules! debug { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Debug, $($arg)+) }; }
#[macro_export]
#[doc(hidden)]
macro_rules! trace { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Trace, $($arg)+) }; }
