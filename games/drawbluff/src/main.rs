//! drawbluff: a Drawful-style party game on tvpipe, in one binary.
//!
//! ```text
//! drawbluff --to "Living Room"              the TV shows video frames drawn here
//! drawbluff --to "Living Room" --web-screen the TV shows the web screen (tvpipe's own browser)
//! drawbluff --to browser --open             a browser tab instead of a TV
//! drawbluff                                 no TV: open the printed screen link yourself (HDMI)
//! drawbluff --to browser --bots 3 --fast    alone, with three fake players
//! ```

use std::io::IsTerminal;
use std::process::ExitCode;
use std::thread;

use drawbluff::bots::{bots, BotOptions};
use drawbluff::game::{Drawbluff, GameEvent, GameOptions, Timings};
use drawbluff::tv::{Tv, TvOptions};
use tvpipe::TunnelProvider;
use tvpipe_party::{assets, Assets, Event, Join, Link, Party, PartyOptions};

static PHONE: Assets = assets!(
    "web/phone",
    [
        "index.html",
        "phone.js",
        "strokes.js",
        "style.css",
        "fonts/OFL.txt",
        "fonts/michroma.woff2",
        "fonts/plex-sans.woff2",
        "fonts/sixtyfour.woff2",
    ]
);

static SCREEN: Assets = assets!("web/screen", ["index.html", "screen.css", "screen.js"]);

const USAGE: &str = "\
usage: drawbluff [--to NAME]... [--web-screen] [--tunnel[=auto|cloudflare|opentunnel]]
                 [--lan | --local] [--bots N] [--rounds N] [--fast] [--size WxH]
                 [--open] [--seed N] [-v]

  --to NAME       a TV (as `tvpipe list` names it, or an address; repeatable),
                  `browser` for a browser tab
  --web-screen    the TV shows the web screen (needs Chrome, Edge, Chromium,
                  Brave or Firefox) instead of video drawn here
  --tunnel=P      phones join through a public link (default: auto, falling
                  back to the LAN)
  --lan           phones join on this network (http://IP:7420)
  --local         phones on this computer only (--no-tunnel)
  --bots N        fake players
  --rounds N      rounds per game (default 2)
  --fast          short timers
  --size WxH      the video size (default 1280x720)
  --open          open the `browser` target's page
  --seed N        the shuffles' seed
  -v, --verbose   say what happens";

#[derive(Debug)]
struct Args {
    to: Vec<String>,
    web_screen: bool,
    join: Join,
    bots: usize,
    rounds: u32,
    fast: bool,
    size: (u32, u32),
    open: bool,
    seed: Option<u32>,
    verbose: bool,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut a = Args {
        to: Vec::new(),
        web_screen: false,
        join: Join::Auto,
        bots: 0,
        rounds: 2,
        fast: false,
        size: (1280, 720),
        open: false,
        seed: None,
        verbose: false,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs {what}"))
        };
        match name.as_str() {
            "-h" | "--help" => return Ok(None),
            "--to" => a.to.push(value("a receiver")?),
            "--web-screen" => a.web_screen = true,
            "--video" => a.web_screen = false,
            "--tunnel" => {
                a.join = match inline.as_deref() {
                    None | Some("auto") => Join::Auto,
                    Some(p) => Join::Tunnel(
                        TunnelProvider::parse(p).ok_or_else(|| format!("unknown tunnel {p:?}"))?,
                    ),
                }
            }
            "--lan" => a.join = Join::Lan(None),
            "--local" | "--no-tunnel" => a.join = Join::Local,
            "--bots" => a.bots = number(&value("a number")?, 0, 7)?,
            "--rounds" => a.rounds = number(&value("a number")?, 1, 10)?,
            "--fast" => a.fast = true,
            "--size" => {
                let v = value("WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size is WxH, e.g. 1280x720")?;
                let (w, h) = (number(w, 320, 3840)?, number(h, 180, 2160)?);
                if w % 2 != 0 || h % 2 != 0 {
                    return Err("--size needs even numbers".into());
                }
                a.size = (w, h);
            }
            "--open" => a.open = true,
            "--seed" => a.seed = Some(number(&value("a number")?, 0, u32::MAX)?),
            "-v" | "--verbose" => a.verbose = true,
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Some(a))
}

fn number<T: std::str::FromStr + PartialOrd + std::fmt::Display>(
    s: &str,
    lo: T,
    hi: T,
) -> Result<T, String> {
    match s.parse::<T>() {
        Ok(n) if n >= lo && n <= hi => Ok(n),
        _ => Err(format!("{s:?}: expected a number from {lo} to {hi}")),
    }
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("drawbluff: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let options = PartyOptions::new()
        .phone(&PHONE)
        .screen(&SCREEN)
        .join(args.join)
        .max_players(8);
    let mut party = match Party::start(options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("drawbluff: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut game = Drawbluff::new(GameOptions {
        rounds: args.rounds,
        timings: if args.fast {
            Timings::FAST
        } else {
            Timings::DEFAULT
        },
        seed: args.seed,
        ..GameOptions::default()
    });
    game.open(&mut party);
    party.flush();

    println!("\n  drawbluff, room {}\n", party.code());
    println!("  phones join at  {}", party.join_url());
    if party.link() == Link::Lan {
        println!("                  (phones must be on the same Wi-Fi as this computer)");
    }
    if let Some(url) = party.watch_url() {
        println!("  watch from afar {url}");
    }
    println!("  the screen      {}\n", party.screen_url());
    if let Some(qr) = party.join_qr() {
        if std::io::stdout().is_terminal() {
            print!("{}", terminal_qr(qr));
        }
    }

    let tv = (!args.to.is_empty()).then(|| {
        let o = TvOptions {
            to: args.to.clone(),
            width: args.size.0,
            height: args.size.1,
            open: args.open,
            verbose: args.verbose,
            ..TvOptions::default()
        };
        if args.web_screen {
            Tv::web(party.screen_url(), o)
        } else {
            Tv::video(party.screen_watch(), o)
        }
    });
    if tv.is_some() {
        let kind = if args.web_screen { "web page" } else { "video" };
        println!("  on the TV ({kind})");
    }
    if args.bots > 0 {
        let mut o = BotOptions::new(party.port(), party.code(), party.join_key());
        o.auto_start = true;
        o.think = (500, 3500);
        o.seed = args.seed.unwrap_or(1);
        let n = args.bots;
        // They join while the loop below answers them.
        thread::spawn(move || match bots(n, &o) {
            Ok(b) => {
                println!("  {n} bot{} joined", if n == 1 { "" } else { "s" });
                // They play until the process ends.
                std::mem::forget(b);
            }
            Err(e) => eprintln!("drawbluff: bots: {e}"),
        });
    }

    let mut shown_url = false;
    loop {
        for event in party.wait_until(game.wake(&party)) {
            if args.verbose {
                log(&party, &event);
            }
            game.event(&mut party, event);
        }
        game.tick(&mut party);
        for e in game.take_events() {
            match e {
                GameEvent::Phase(p) if args.verbose => eprintln!("drawbluff: {p}"),
                GameEvent::End(table) => {
                    let line: Vec<String> = table
                        .iter()
                        .map(|l| format!("{} {}", l.name, l.score))
                        .collect();
                    println!("  game over: {}", line.join(", "));
                }
                GameEvent::Phase(_) => {}
            }
        }
        party.flush();
        if !shown_url {
            if let Some(url) = tv.as_ref().and_then(Tv::player_url) {
                println!("  the TV's page  {url}");
                shown_url = true;
            }
        }
    }
}

fn log(party: &Party, e: &Event) {
    let name = |id| party.player(id).map_or("?", |p| p.name()).to_owned();
    match e {
        Event::Joined(id) => eprintln!("drawbluff: + {}", name(id)),
        Event::Back(id) => eprintln!("drawbluff: {} is back", name(id)),
        Event::Away(id) => eprintln!("drawbluff: {} is away", name(id)),
        Event::Left { name, reason, .. } => eprintln!("drawbluff: - {name} ({reason})"),
        Event::Screen { id, connected, .. } => eprintln!(
            "drawbluff: screen {id} {}",
            if *connected { "connected" } else { "left" }
        ),
        _ => {}
    }
}

/// A QR code in the terminal: two module rows per line, half blocks,
/// black on white.
fn terminal_qr(qr: &tvpipe::QrCode) -> String {
    let quiet = 2isize;
    let n = qr.size() as isize;
    let dark = |x: isize, y: isize| {
        let (x, y) = (x - quiet, y - quiet);
        (0..n).contains(&x) && (0..n).contains(&y) && qr.dark(x as usize, y as usize)
    };
    let size = n + 2 * quiet;
    let mut out = String::new();
    for y in (0..size).step_by(2) {
        out.push_str("  \x1b[30;47m");
        for x in 0..size {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pages_match_the_directories() {
        PHONE.check_dir().unwrap();
        SCREEN.check_dir().unwrap();
    }

    fn args(s: &str) -> Result<Option<Args>, String> {
        parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn options() {
        let a = args("--to TV --to browser --tunnel=cloudflare --bots 3 --size 640x360 --fast")
            .unwrap()
            .unwrap();
        assert_eq!(a.to, ["TV", "browser"]);
        assert!(matches!(a.join, Join::Tunnel(TunnelProvider::Cloudflare)));
        assert_eq!((a.bots, a.size, a.fast), (3, (640, 360), true));
        assert!(matches!(
            args("--lan").unwrap().unwrap().join,
            Join::Lan(None)
        ));
        assert!(matches!(
            args("--no-tunnel").unwrap().unwrap().join,
            Join::Local
        ));
        assert!(matches!(
            args("--tunnel").unwrap().unwrap().join,
            Join::Auto
        ));
        assert!(args("--help").unwrap().is_none());
        for bad in [
            "--size 641x360",
            "--rounds 0",
            "--bots",
            "--tunnel=nope",
            "--what",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }
}
