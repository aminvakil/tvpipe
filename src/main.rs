//! tvpipe: low-latency mirroring to AirPlay and Google Cast receivers.
//!
//! The command-line tool: argument parsing, Ctrl-C and `q`, and the
//! machine interface, on top of the library.

#[macro_use]
extern crate tvpipe;

mod cli;
#[path = "util/interrupt.rs"]
mod interrupt;
mod machine;

use tvpipe::__cli::{error, events};
use tvpipe::log;

use std::process::ExitCode;

fn main() -> ExitCode {
    let (global, command) = cli::parse_env();
    let level = if global.debug {
        log::Level::Trace
    } else if global.verbose {
        log::Level::Debug
    } else {
        log::Level::Info
    };
    let installed = if global.machine {
        log::set_logger(level, |r| machine::emit(&events::Event::Log(r.clone())))
    } else {
        let start = std::time::Instant::now();
        log::set_logger(level, move |r| {
            use std::io::Write;
            let t = start.elapsed();
            let _ = writeln!(
                std::io::stderr().lock(),
                "{:>4}.{:03} {} {}: {}",
                t.as_secs(),
                t.subsec_millis(),
                r.level.label(),
                r.module,
                r.message
            );
        })
    };
    // Only fails when installed twice, which main does not.
    let _ = installed;
    let mut ctx = tvpipe::__cli::Ctx::new();
    if global.machine {
        ctx.events = machine::events();
        machine::hello(command.name());
        machine::start_router(ctx.stop.clone());
    }
    // Fine-grained timed waits for pacing (Windows; `util::timer`).
    tvpipe::__cli::raise_resolution();
    // Ctrl-C and `q` stop sessions (not discover/pair, which end by
    // themselves; pair reads its PIN from stdin).
    let (session, reads_stdin) = match &command {
        tvpipe::__cli::opts::Command::Play(o) | tvpipe::__cli::opts::Command::TestPattern(o) => {
            (true, o.input == tvpipe::__cli::opts::InputSpec::Stdin)
        }
        tvpipe::__cli::opts::Command::Share(_) | tvpipe::__cli::opts::Command::Screen(_) => {
            (true, false)
        }
        _ => (false, false),
    };
    // A person at a terminal gets the public links as QR codes too, after
    // the log line naming them (`--no-qr`, `TVPIPE_NO_QR`).
    if session && !global.machine && tvpipe::__cli::qr::wanted(global.no_qr) {
        ctx.events = events::Events::to(|e| {
            if let events::Event::Tunnel(t) = e {
                tvpipe::__cli::qr::print(&t.url);
            }
        });
    }
    if session {
        if interrupt::install() {
            interrupt::link(ctx.stop.clone());
        } else {
            warn!("cannot catch Ctrl-C; receivers will not get a clean stop");
        }
        // With --machine, stdin belongs to the machine interface's router.
        if !reads_stdin && !global.machine {
            interrupt::watch_stdin(ctx.stop.clone());
        }
    }
    let io = tvpipe::__cli::GameIo {
        // With --machine, the stdin router hands SLRW over and keyframe
        // requests become events.
        stdin: machine::stdin_reader(),
        stdout: if global.machine {
            Box::new(machine::KeyframeEvents::new(ctx.events.clone()))
        } else {
            Box::new(std::io::stdout())
        },
        approve: Box::new(approve_in_terminal),
    };
    let rv = match command {
        tvpipe::__cli::opts::Command::Pair { target, pin } => {
            tvpipe::__cli::pair(&target, pin.as_deref(), &ctx, &mut |req| prompt_pin(req))
        }
        command => tvpipe::__cli::run(command, &ctx, io),
    };
    // A stream the machine interface refused is the command's error.
    let rv = match (rv, machine::failure()) {
        (_, Some(e)) => Err(e),
        // After a stop request (`q`, the machine interface's `stop` or
        // stdin closing) an error is its consequence, e.g. a receiver
        // that was still connecting.
        (Err(e), None) if ctx.stop.requested() && !interrupt::signalled() => {
            debug!("after the stop request: {e}");
            Ok(())
        }
        (rv, None) => rv,
    };
    let report = |err: &error::Error| {
        if global.machine {
            machine::emit(&machine::error_event(err));
        } else {
            eprintln!("error: {err}");
        }
    };
    // A Ctrl-C/SIGTERM stop exits with 130 like other CLI tools, after the
    // receivers got their clean teardown.
    if interrupt::signalled() {
        match rv {
            Err(err) => report(&err),
            Ok(()) => machine::end_if_unsent(events::EndReason::Stopped),
        }
        return ExitCode::from(130);
    }
    match rv {
        Ok(()) => {
            machine::end_if_unsent(if ctx.stop.requested() {
                events::EndReason::Stopped
            } else {
                events::EndReason::Done
            });
            ExitCode::SUCCESS
        }
        Err(err) => {
            report(&err);
            ExitCode::from(1)
        }
    }
}

/// Reads the PIN from stdin after prompting on stderr; with `--machine`,
/// the router's `pin` command answers the `pin-required` event.
fn prompt_pin(req: &events::PinRequest) -> error::Result<String> {
    use std::io::{BufRead, Write};
    if let Some(router) = machine::router() {
        return router.wait_pin();
    }
    eprint!("Enter the PIN shown on {}: ", req.name);
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| error::Error::io("read PIN", e))?;
    if line.is_empty() {
        return Err(error::Error::usage("no PIN entered"));
    }
    Ok(line.trim().to_owned())
}

/// Asks on the terminal (`y` + Enter) whether a share from over the
/// tunnel may reach the receivers. Without a terminal nothing remote is
/// shown unless `--yes` was given.
fn approve_in_terminal(what: &str, cancel: &dyn Fn() -> bool) -> bool {
    let _ = what; // logged by the session
    match interrupt::ask("share: show it on the receivers? [y/N]", cancel) {
        Some(a) => matches!(a.trim(), "y" | "Y" | "yes" | "Yes" | "YES"),
        None => {
            if !cancel() {
                warn!("share: no terminal to ask on; pass --yes to show remote shares unasked");
            }
            false
        }
    }
}
