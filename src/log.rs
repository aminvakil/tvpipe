//! Tiny leveled logger to stderr.
//!
//! Use the crate macros: `error!`, `warn!`, `info!`, `debug!`, `trace!`.
//! The default level is `Info`; `-v` raises it to `Debug`, `--debug` to
//! `Trace`.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static START: OnceLock<Instant> = OnceLock::new();

pub fn set_level(level: Level) {
    START.get_or_init(Instant::now);
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

#[doc(hidden)]
pub fn write(level: Level, module: &str, args: std::fmt::Arguments<'_>) {
    let elapsed = START.get_or_init(Instant::now).elapsed();
    let module = module.strip_prefix("tvpipe::").unwrap_or(module);
    let mut err = std::io::stderr().lock();
    let _ = writeln!(
        err,
        "{:>4}.{:03} {} {}: {}",
        elapsed.as_secs(),
        elapsed.subsec_millis(),
        level.label(),
        module,
        args
    );
}

#[macro_export]
macro_rules! log_at {
    ($level:expr, $($arg:tt)+) => {
        if $crate::log::enabled($level) {
            $crate::log::write($level, module_path!(), format_args!($($arg)+));
        }
    };
}

#[macro_export]
macro_rules! error { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Error, $($arg)+) }; }
#[macro_export]
macro_rules! warn { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Warn, $($arg)+) }; }
#[macro_export]
macro_rules! info { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Info, $($arg)+) }; }
#[macro_export]
macro_rules! debug { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Debug, $($arg)+) }; }
#[macro_export]
macro_rules! trace { ($($arg:tt)+) => { $crate::log_at!($crate::log::Level::Trace, $($arg)+) }; }
