//! tvpipe: low-latency mirroring to AirPlay and Google Cast receivers.
//!
//! The command-line tool: argument parsing, Ctrl-C and `q`, and the
//! machine interface, on top of the library.

#[macro_use]
extern crate tvpipe;

mod cli;
mod interrupt;
mod machine;

use tvpipe::__cli::{error, events};
use tvpipe::log;

use std::process::ExitCode;

use tvpipe::__cli::opts::{Command, InputSpec, Provider, QrFormat};
use tvpipe::{Device, Storage};

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
    // The CLI shares one config directory with every run (and with
    // programs that use Storage::cli()).
    let mut ctx = tvpipe::__cli::Ctx::new();
    ctx.storage = Storage::cli();
    if global.machine {
        ctx.events = machine::events();
        machine::hello(command.name());
        machine::start_router(ctx.stop.clone());
    }
    // Ctrl-C and `q` stop sessions (not discover/pair, which end by
    // themselves; pair reads its PIN from stdin).
    let (session, reads_stdin) = match &command {
        Command::Play(o) | Command::TestPattern(o) => (true, o.input == InputSpec::Stdin),
        Command::Share(_) | Command::Screen(_) | Command::Tunnel { .. } => (true, false),
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
    let stop = ctx.stop.clone();
    let rv = run(command, ctx, &global);
    // A stream the machine interface refused is the command's error.
    let rv = match (rv, machine::failure()) {
        (_, Some(e)) => Err(e),
        // After a stop request (`q`, the machine interface's `stop` or
        // stdin closing) an error is its consequence, e.g. a receiver
        // that was still connecting.
        (Err(e), None) if stop.requested() && !interrupt::signalled() => {
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
            Ok(()) if global.machine => machine::end_if_unsent(events::EndReason::Stopped),
            Ok(()) => {}
        }
        return ExitCode::from(130);
    }
    match rv {
        Ok(()) if !global.machine => ExitCode::SUCCESS,
        Ok(()) => {
            machine::end_if_unsent(if stop.requested() {
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

/// Runs one command: discovery and pairing through the library's
/// functions, everything else as a library session.
fn run(command: Command, ctx: tvpipe::__cli::Ctx, global: &cli::Global) -> error::Result<()> {
    match command {
        Command::Discover {
            timeout,
            json,
            interfaces,
        } => {
            let devices = tvpipe::__cli::discover(timeout, &interfaces, &ctx.storage)?;
            if global.machine {
                machine::emit(&tvpipe::__cli::devices_event(devices));
                return Ok(());
            }
            print(&if json {
                tvpipe::__cli::devices_json(&devices)
            } else {
                table(&devices)
            })
        }
        Command::Pair { target, pin } => {
            let p = tvpipe::__cli::pair(&target, pin.as_deref(), &ctx, &mut prompt_pin)?;
            if !global.machine {
                print(&format!(
                    "Paired with {} ({})\n",
                    p.name,
                    p.model.as_deref().unwrap_or("unknown model")
                ))?;
            }
            Ok(())
        }
        Command::Unpair { target } => {
            let options = tvpipe::Options::new().storage(ctx.storage.clone());
            let removed = tvpipe::unpair(&target, &options)?;
            for u in removed {
                if global.machine {
                    machine::emit(&events::Event::Unpaired(u));
                } else {
                    print(&format!("Forgot pairing with {} ({})\n", u.name, u.id))?;
                }
            }
            Ok(())
        }
        Command::Tunnel { port, provider } => tunnel(port, provider, &ctx),
        Command::Qr { .. } if global.machine => Err(error::Error::usage(
            "qr writes the code to stdout: run it without --machine",
        )),
        Command::Qr { text, format } => {
            let max = match format {
                QrFormat::Terminal => tvpipe::__cli::qr::TERMINAL_MAX_VERSION,
                _ => tvpipe::__cli::qr::MAX_VERSION,
            };
            let Some(qr) = tvpipe::__cli::qr::encode(text.as_bytes(), max) else {
                return Err(error::Error::usage(format!(
                    "qr: {} bytes do not fit a QR code{}",
                    text.len(),
                    if format == QrFormat::Terminal {
                        " for the terminal (try --svg)"
                    } else {
                        ""
                    }
                )));
            };
            print(&match format {
                QrFormat::Terminal => tvpipe::__cli::qr::render_terminal(&text).unwrap_or_default(),
                QrFormat::Svg => qr.to_svg(4),
                QrFormat::Rows => qr.rows().join("\n") + "\n",
            })
        }
        command => {
            let io = tvpipe::__cli::GameIo {
                // With --machine, the stdin router hands SLRW over and
                // keyframe requests become events.
                stdin: machine::stdin_reader(),
                stdout: if global.machine {
                    Box::new(machine::KeyframeEvents::new(ctx.events.clone()))
                } else {
                    Box::new(std::io::stdout())
                },
                approve: Box::new(approve_in_terminal),
            };
            tvpipe::__cli::spawn(command, ctx, io, None)?
                .wait()
                .map(|_| ())
        }
    }
}

/// `tunnel PORT`: a public link for 127.0.0.1:PORT until a stop request
/// (Ctrl-C, `q`, the machine interface's `stop` or stdin closing).
fn tunnel(port: u16, provider: Provider, ctx: &tvpipe::__cli::Ctx) -> error::Result<()> {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    if TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_err() {
        warn!("tunnel: nothing answers on 127.0.0.1:{port} yet; visitors get errors until it does");
    }
    let stop = ctx.stop.clone();
    let t = tvpipe::__cli::start_tunnel(port, provider, &|| stop.requested())?;
    let url = format!("https://{}/", t.host());
    info!(
        "tunnel: {url} leads to http://127.0.0.1:{port} ({}; the link is new every run)",
        t.name()
    );
    ctx.events
        .emit(tvpipe::__cli::port_tunnel_event(provider, url));
    while !ctx.stop.wait(std::time::Duration::from_secs(3600)) {}
    info!("tunnel: closed");
    drop(t);
    Ok(())
}

/// Writes to stdout; a closed pipe (`| head`) is not an error.
fn print(text: &str) -> error::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        r => r.map_err(|e| error::Error::io("write the output", e)),
    }
}

/// `discover`'s table: one receiver per line.
fn table(devices: &[Device]) -> String {
    if devices.is_empty() {
        return "no receivers found\n".to_owned();
    }
    let protocols = |d: &Device| {
        let mut p = Vec::new();
        if let Some(a) = &d.airplay {
            p.push(if a.mirroring {
                "airplay(mirror)"
            } else {
                "airplay"
            });
        }
        if d.raop.is_some() {
            p.push("raop");
        }
        if let Some(c) = &d.cast {
            p.push(if c.video { "cast(video)" } else { "cast" });
        }
        p.join(",")
    };
    let rows: Vec<[String; 4]> = devices
        .iter()
        .map(|d| {
            [
                d.name.clone(),
                d.addresses
                    .first()
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                protocols(d),
                d.model.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let header = ["NAME", "ADDRESS", "PROTOCOLS", "MODEL"].map(str::to_owned);
    let mut widths = [0usize; 4];
    for row in std::iter::once(&header).chain(&rows) {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in std::iter::once(&header).chain(&rows) {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if i + 1 < row.len() {
                let pad = widths[i] - cell.chars().count() + 2;
                line.extend(std::iter::repeat_n(' ', pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
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
