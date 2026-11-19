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
mod ctx;
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
    let ctx = ctx::Ctx::new();
    if global.machine {
        machine::enable();
        machine::hello(command.name());
        machine::start_router(ctx.stop.clone());
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
    // Ctrl-C and `q` stop sessions (not discover/pair, which end by
    // themselves; pair reads its PIN from stdin).
    let (session, reads_stdin) = match &command {
        cli::Command::Play(o) | cli::Command::TestPattern(o) => {
            (true, o.input == cli::InputSpec::Stdin)
        }
        cli::Command::Share(_) | cli::Command::Screen(_) => (true, false),
        _ => (false, false),
    };
    if session {
        if util::interrupt::install() {
            util::interrupt::link(ctx.stop.clone());
        } else {
            warn!("cannot catch Ctrl-C; receivers will not get a clean stop");
        }
        // With --machine, stdin belongs to the machine interface's router.
        if !reads_stdin && !global.machine {
            util::interrupt::watch_stdin(ctx.stop.clone());
        }
    }
    let rv = session::run(command, &ctx);
    // A stream the machine interface refused is the command's error.
    let rv = match (rv, machine::failure()) {
        (_, Some(e)) => Err(e),
        // After a stop request (`q`, the machine interface's `stop` or
        // stdin closing) an error is its consequence, e.g. a receiver
        // that was still connecting.
        (Err(e), None) if ctx.stop.requested() && !util::interrupt::signalled() => {
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
            machine::end_if_unsent(if ctx.stop.requested() {
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
