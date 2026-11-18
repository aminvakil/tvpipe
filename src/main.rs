//! tvpipe: low-latency mirroring to AirPlay and Google Cast receivers.

// Modules are filled in by the workstreams; until they are wired into the
// CLI, much of their API is unused from `main`.
#![allow(dead_code)]

#[macro_use]
mod log;

mod airplay;
mod browser;
mod cast;
mod cli;
mod codec;
mod crypto;
mod error;
mod machine;
mod media;
mod net;
mod session;
mod types;
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    let (global, command) = cli::parse_env();
    if global.machine {
        machine::enable();
        machine::hello(command.name());
        machine::start_router();
    }
    log::set_level(if global.debug {
        log::Level::Trace
    } else if global.verbose {
        log::Level::Debug
    } else {
        log::Level::Info
    });
    // Fine-grained timed waits for pacing (Windows; `util::timer`).
    util::timer::raise_resolution();
    let rv = session::run(command);
    // A stream the machine interface refused is the command's error.
    let rv = match (rv, machine::failure()) {
        (_, Some(e)) => Err(e),
        // After a stop request (`q`, the machine interface's `stop` or
        // stdin closing) an error is its consequence, e.g. a receiver
        // that was still connecting.
        (Err(e), None) if util::interrupt::requested() && !util::interrupt::signalled() => {
            debug!("after the stop request: {e}");
            Ok(())
        }
        (rv, None) => rv,
    };
    let report = |err: &error::Error| {
        if global.machine {
            machine::emit(machine::error_event(err));
        } else {
            eprintln!("error: {err}");
        }
    };
    // A Ctrl-C/SIGTERM stop exits with 130 like other CLI tools, after the
    // receivers got their clean teardown.
    if util::interrupt::signalled() {
        match rv {
            Err(err) => report(&err),
            Ok(()) => machine::end_if_unsent("stopped"),
        }
        return ExitCode::from(130);
    }
    match rv {
        Ok(()) => {
            machine::end_if_unsent(if util::interrupt::requested() {
                "stopped"
            } else {
                "done"
            });
            ExitCode::SUCCESS
        }
        Err(err) => {
            report(&err);
            ExitCode::from(1)
        }
    }
}
