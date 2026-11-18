//! tvpipe: low-latency mirroring to AirPlay and Google Cast receivers.

// Modules are filled in by the workstreams; until they are wired into the
// CLI, much of their API is unused from `main`.
#![allow(dead_code)]

#[macro_use]
mod log;

mod airplay;
mod cast;
mod cli;
mod codec;
mod crypto;
mod error;
mod media;
mod net;
mod session;
mod types;
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    let (global, command) = cli::parse_env();
    log::set_level(if global.debug {
        log::Level::Trace
    } else if global.verbose {
        log::Level::Debug
    } else {
        log::Level::Info
    });
    // Fine-grained timed waits for pacing (Windows; decision 0011).
    util::timer::raise_resolution();
    let rv = session::run(command);
    // A Ctrl-C/SIGTERM stop exits with 130 like other CLI tools, after the
    // receivers got their clean teardown.
    if util::interrupt::signalled() {
        if let Err(err) = rv {
            eprintln!("error: {err}");
        }
        return ExitCode::from(130);
    }
    match rv {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}
