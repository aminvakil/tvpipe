//! `tvpipe screen --engine ffmpeg`: ffmpeg's input arguments that capture
//! a screen or a window on each OS (strings for ffmpeg, no OS API).
//!
//! | OS | `--display N` | `--window TITLE` | Sound |
//! |---|---|---|---|
//! | macOS | `avfoundation` "Capture screen N-1" | — | — (needs a virtual device) |
//! | Windows | `ddagrab` (Desktop Duplication) output N-1 | `gdigrab` `title=` (the whole title) | — (no WASAPI loopback in ffmpeg) |
//! | Linux (X11) | `x11grab`, the monitor's area from `xrandr --listmonitors` | `x11grab -window_id` from `xwininfo -root -tree` | PulseAudio/PipeWire `@DEFAULT_MONITOR@` |
//!
//! Wayland sessions are refused (`x11grab` sees only X11 clients; the
//! browser engine asks the desktop portal instead). The OS is a string
//! (`std::env::consts::OS`) and the helper programs' output is passed in,
//! so every OS's arguments are tested everywhere.
//!
//! Sources: ffmpeg's device documentation (avfoundation, gdigrab,
//! x11grab, pulse) and filter documentation (ddagrab, hwdownload);
//! PulseAudio's special source names (`@DEFAULT_MONITOR@`).

use crate::cli::ScreenTarget;
use crate::error::{Error, Result};

/// ffmpeg's input arguments and what the user should know about them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenInput {
    pub args: Vec<String>,
    /// Whether the arguments capture sound.
    pub audio: bool,
    /// Warnings to show (no sound on this OS, ...).
    pub notes: Vec<String>,
}

/// What the arguments are built from.
pub struct Env<'a> {
    /// `std::env::consts::OS`.
    pub os: &'a str,
    /// An environment variable.
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// Runs a helper program (`xrandr`, `xwininfo`): its stdout, `None`
    /// if it cannot run.
    pub run: &'a dyn Fn(&str, &[&str]) -> Option<String>,
}

/// ffmpeg input arguments that capture `target` at `fps`, with the
/// system's sound when `audio` and the OS allows.
pub fn ffmpeg_input(
    env: &Env,
    target: &ScreenTarget,
    fps: u32,
    audio: bool,
) -> Result<ScreenInput> {
    let fps = fps.clamp(1, 120).to_string();
    match env.os {
        "macos" => macos(target, &fps, audio),
        "windows" => windows(target, &fps, audio),
        "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" => x11(env, target, &fps, audio),
        os => Err(Error::unsupported(format!(
            "--engine ffmpeg: no screen capture arguments for {os}"
        ))),
    }
}

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn macos(target: &ScreenTarget, fps: &str, audio: bool) -> Result<ScreenInput> {
    let device = match target {
        ScreenTarget::Display(n) => format!("Capture screen {}", n - 1),
        // `ffmpeg -f avfoundation -list_devices true -i ""` names them.
        ScreenTarget::DisplayNamed(name) => name.clone(),
        ScreenTarget::Window(_) => {
            return Err(Error::unsupported(
                "ffmpeg cannot capture a single window on macOS; use --engine browser",
            ))
        }
        ScreenTarget::Pick => return Err(Error::usage("--pick needs --engine browser")),
    };
    let mut args = strings(&[
        "-f",
        "avfoundation",
        "-capture_cursor",
        "1",
        "-framerate",
        fps,
        "-i",
    ]);
    args.push(format!("{device}:none"));
    let mut notes = Vec::new();
    if audio {
        notes.push(
            "ffmpeg cannot record the computer's sound on macOS (it needs a virtual audio \
             device such as BlackHole); streaming the screen without sound. --engine browser \
             has it"
                .to_owned(),
        );
    }
    Ok(ScreenInput {
        args,
        audio: false,
        notes,
    })
}

fn windows(target: &ScreenTarget, fps: &str, audio: bool) -> Result<ScreenInput> {
    let args = match target {
        // Desktop Duplication (Windows 8+, D3D11), copied to system memory
        // for the encoders.
        ScreenTarget::Display(n) => vec![
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!(
                "ddagrab=output_idx={}:framerate={fps}:draw_mouse=1,hwdownload,format=bgra",
                n - 1
            ),
        ],
        ScreenTarget::DisplayNamed(_) => {
            return Err(Error::usage(
                "--engine ffmpeg on Windows takes --display N (1, 2, ...), not a name",
            ))
        }
        // gdigrab wants the window's whole title.
        ScreenTarget::Window(title) => {
            let mut a = strings(&["-f", "gdigrab", "-framerate", fps, "-draw_mouse", "1", "-i"]);
            a.push(format!("title={title}"));
            a
        }
        ScreenTarget::Pick => return Err(Error::usage("--pick needs --engine browser")),
    };
    let mut notes = Vec::new();
    if audio {
        notes.push(
            "ffmpeg cannot record the computer's sound on Windows (no WASAPI loopback); \
             streaming without sound. --engine browser has it"
                .to_owned(),
        );
    }
    Ok(ScreenInput {
        args,
        audio: false,
        notes,
    })
}

/// A monitor from `xrandr --listmonitors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

/// Parses `xrandr --listmonitors`:
///
/// ```text
/// Monitors: 2
///  0: +*eDP-1 1920/344x1080/194+0+0  eDP-1
///  1: +HDMI-1 2560/597x1440/336+1920+0  HDMI-1
/// ```
pub fn parse_monitors(out: &str) -> Vec<Monitor> {
    let mut v = Vec::new();
    for line in out
        .lines()
        .skip_while(|l| !l.starts_with("Monitors:"))
        .skip(1)
    {
        let mut f = line.split_whitespace();
        let (Some(_index), Some(flags_name), Some(geom)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let primary = flags_name.trim_start_matches('+').starts_with('*');
        let name = f
            .next()
            .unwrap_or_else(|| flags_name.trim_start_matches(['+', '*']))
            .to_owned();
        if let Some(m) = parse_geometry(geom) {
            v.push(Monitor {
                name,
                primary,
                width: m.0,
                height: m.1,
                x: m.2,
                y: m.3,
            });
        }
    }
    v
}

/// `W/mmxH/mm+X+Y` → (W, H, X, Y).
fn parse_geometry(g: &str) -> Option<(u32, u32, i32, i32)> {
    let (w, rest) = g.split_once('x')?;
    let w = w.split('/').next()?.parse().ok()?;
    let h_end = rest.find(['+', '-'])?;
    let h = rest[..h_end].split('/').next()?.parse().ok()?;
    // `+X+Y`, where X and Y may be negative (`+-1280+0`).
    let pos = &rest[h_end..];
    let pos = pos.strip_prefix('+').unwrap_or(pos);
    let split = pos.get(1..)?.find(['+', '-'])? + 1;
    let x = pos[..split].parse().ok()?;
    let y = pos[split..].trim_start_matches('+').parse().ok()?;
    Some((w, h, x, y))
}

/// The window id of the first window whose title contains `title`, from
/// `xwininfo -root -tree`:
///
/// ```text
///      0x3a00003 "Slides - Impress": ("libreoffice" "libreoffice")  1920x1080+0+0  +0+0
/// ```
pub fn find_window(tree: &str, title: &str) -> Option<String> {
    tree.lines().find_map(|line| {
        let line = line.trim_start();
        let (id, rest) = line.split_once(' ')?;
        if !id.starts_with("0x") {
            return None;
        }
        let name = rest.strip_prefix('"')?;
        let end = name.find("\":")?;
        name[..end].contains(title).then(|| id.to_owned())
    })
}

fn x11(env: &Env, target: &ScreenTarget, fps: &str, audio: bool) -> Result<ScreenInput> {
    let display = (env.var)("DISPLAY").filter(|d| !d.is_empty());
    let wayland = (env.var)("WAYLAND_DISPLAY").is_some_and(|w| !w.is_empty())
        || (env.var)("XDG_SESSION_TYPE").as_deref() == Some("wayland");
    if wayland {
        return Err(Error::unsupported(
            "a Wayland session: ffmpeg's x11grab sees only X11 windows. Use --engine browser \
             (it asks through the desktop's screen-sharing portal)",
        ));
    }
    let Some(display) = display else {
        return Err(Error::unsupported(
            "no X11 display ($DISPLAY is not set); use --engine browser",
        ));
    };
    let mut args = strings(&["-f", "x11grab", "-framerate", fps, "-draw_mouse", "1"]);
    match target {
        ScreenTarget::Window(title) => {
            let tree = (env.run)("xwininfo", &["-root", "-tree"]).ok_or_else(|| {
                Error::not_found("--window with --engine ffmpeg needs xwininfo (x11-utils)")
            })?;
            let id = find_window(&tree, title).ok_or_else(|| {
                Error::not_found(format!("no window whose title contains \"{title}\""))
            })?;
            args.extend(["-window_id".into(), id, "-i".into(), display]);
        }
        ScreenTarget::Display(_) | ScreenTarget::DisplayNamed(_) => {
            let monitors = (env.run)("xrandr", &["--listmonitors"])
                .map(|o| parse_monitors(&o))
                .unwrap_or_default();
            let chosen = match target {
                ScreenTarget::Display(1) => monitors
                    .iter()
                    .find(|m| m.primary)
                    .or_else(|| monitors.first()),
                ScreenTarget::Display(n) => monitors.get(*n as usize - 1),
                ScreenTarget::DisplayNamed(name) => monitors.iter().find(|m| m.name == *name),
                _ => None,
            };
            match (chosen, target) {
                // One monitor (or no xrandr): the whole X screen.
                (Some(_), ScreenTarget::Display(1)) | (None, ScreenTarget::Display(1))
                    if monitors.len() <= 1 =>
                {
                    args.extend(["-i".into(), display]);
                }
                (Some(m), _) => {
                    args.extend([
                        "-video_size".into(),
                        format!("{}x{}", m.width, m.height),
                        "-i".into(),
                        format!("{display}+{},{}", m.x, m.y),
                    ]);
                }
                (None, _) => {
                    let names: Vec<&str> = monitors.iter().map(|m| m.name.as_str()).collect();
                    return Err(Error::not_found(if monitors.is_empty() {
                        "--display needs xrandr to find the monitors (or use 1: the whole X screen)"
                            .to_owned()
                    } else {
                        format!("no such monitor; xrandr lists {}", names.join(", "))
                    }));
                }
            }
        }
        ScreenTarget::Pick => return Err(Error::usage("--pick needs --engine browser")),
    }
    if audio {
        // The monitor of the default output (PulseAudio, and PipeWire's
        // pulse server).
        args.extend(strings(&[
            "-f",
            "pulse",
            "-thread_queue_size",
            "64",
            "-i",
            "@DEFAULT_MONITOR@",
        ]));
    }
    Ok(ScreenInput {
        args,
        audio,
        notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = "Monitors: 2\n 0: +*eDP-1 1920/344x1080/194+0+0  eDP-1\n \
                            1: +HDMI-1 2560/597x1440/336+1920+0  HDMI-1\n";
    const TREE: &str = "\nxwininfo: Window id: 0x1e7 (the root window) (has no name)\n\n  \
        Root window id: 0x1e7 (the root window) (has no name)\n  Parent window id: 0x0 (none)\n     \
        2 children:\n     0x3a00003 \"Slides - Impress\": (\"libreoffice\" \"libreoffice\")  \
        1920x1080+0+0  +0+0\n     0x2800001 (has no name): ()  1x1+-1+-1  +-1+-1\n";

    fn env<'a>(
        os: &'a str,
        run: &'a dyn Fn(&str, &[&str]) -> Option<String>,
        var: &'a dyn Fn(&str) -> Option<String>,
    ) -> Env<'a> {
        Env { os, var, run }
    }

    fn x11_env(prog: &str, args: &[&str]) -> Option<String> {
        match (prog, args) {
            ("xrandr", ["--listmonitors"]) => Some(MONITORS.into()),
            ("xwininfo", ["-root", "-tree"]) => Some(TREE.into()),
            _ => None,
        }
    }

    fn no_run(_: &str, _: &[&str]) -> Option<String> {
        None
    }

    fn joined(i: &ScreenInput) -> String {
        i.args.join(" ")
    }

    #[test]
    fn macos_args() {
        let var = |_: &str| None;
        let e = env("macos", &no_run, &var);
        let i = ffmpeg_input(&e, &ScreenTarget::Display(1), 30, true).unwrap();
        assert_eq!(
            i.args,
            [
                "-f",
                "avfoundation",
                "-capture_cursor",
                "1",
                "-framerate",
                "30",
                "-i",
                "Capture screen 0:none"
            ]
        );
        assert!(!i.audio);
        assert_eq!(i.notes.len(), 1);
        let i = ffmpeg_input(&e, &ScreenTarget::Display(2), 60, false).unwrap();
        assert!(joined(&i).ends_with("-framerate 60 -i Capture screen 1:none"));
        assert!(i.notes.is_empty());
        assert!(ffmpeg_input(&e, &ScreenTarget::Window("x".into()), 30, true).is_err());
        assert!(ffmpeg_input(&e, &ScreenTarget::Pick, 30, true).is_err());
    }

    #[test]
    fn windows_args() {
        let var = |_: &str| None;
        let e = env("windows", &no_run, &var);
        let i = ffmpeg_input(&e, &ScreenTarget::Display(2), 30, true).unwrap();
        assert_eq!(
            joined(&i),
            "-f lavfi -i ddagrab=output_idx=1:framerate=30:draw_mouse=1,hwdownload,format=bgra"
        );
        assert!(!i.audio && i.notes.len() == 1);
        let i = ffmpeg_input(
            &e,
            &ScreenTarget::Window("Untitled - Notepad".into()),
            30,
            false,
        )
        .unwrap();
        assert_eq!(
            i.args,
            [
                "-f",
                "gdigrab",
                "-framerate",
                "30",
                "-draw_mouse",
                "1",
                "-i",
                "title=Untitled - Notepad"
            ]
        );
        assert!(ffmpeg_input(&e, &ScreenTarget::DisplayNamed("x".into()), 30, true).is_err());
    }

    #[test]
    fn x11_args() {
        let var = |k: &str| (k == "DISPLAY").then(|| ":1".to_owned());
        let e = env("linux", &x11_env, &var);
        let i = ffmpeg_input(&e, &ScreenTarget::Display(1), 30, true).unwrap();
        assert_eq!(
            joined(&i),
            "-f x11grab -framerate 30 -draw_mouse 1 -video_size 1920x1080 -i :1+0,0 \
             -f pulse -thread_queue_size 64 -i @DEFAULT_MONITOR@"
        );
        assert!(i.audio);
        let i = ffmpeg_input(&e, &ScreenTarget::Display(2), 30, false).unwrap();
        assert_eq!(
            joined(&i),
            "-f x11grab -framerate 30 -draw_mouse 1 -video_size 2560x1440 -i :1+1920,0"
        );
        let i = ffmpeg_input(&e, &ScreenTarget::DisplayNamed("HDMI-1".into()), 30, false).unwrap();
        assert!(joined(&i).ends_with("-i :1+1920,0"));
        assert!(ffmpeg_input(&e, &ScreenTarget::Display(3), 30, false)
            .unwrap_err()
            .to_string()
            .contains("eDP-1, HDMI-1"));
        let i = ffmpeg_input(&e, &ScreenTarget::Window("Impress".into()), 30, false).unwrap();
        assert_eq!(
            joined(&i),
            "-f x11grab -framerate 30 -draw_mouse 1 -window_id 0x3a00003 -i :1"
        );
        assert!(ffmpeg_input(&e, &ScreenTarget::Window("Keynote".into()), 30, false).is_err());

        // Without xrandr: the whole X screen for the first display only.
        let e = env("linux", &no_run, &var);
        let i = ffmpeg_input(&e, &ScreenTarget::Display(1), 30, false).unwrap();
        assert_eq!(joined(&i), "-f x11grab -framerate 30 -draw_mouse 1 -i :1");
        assert!(ffmpeg_input(&e, &ScreenTarget::Display(2), 30, false).is_err());

        // Wayland, or no display at all.
        let wl = |k: &str| match k {
            "DISPLAY" => Some(":0".to_owned()),
            "WAYLAND_DISPLAY" => Some("wayland-0".to_owned()),
            _ => None,
        };
        let e = env("linux", &x11_env, &wl);
        assert!(ffmpeg_input(&e, &ScreenTarget::Display(1), 30, false)
            .unwrap_err()
            .to_string()
            .contains("Wayland"));
        let none = |_: &str| None;
        let e = env("linux", &x11_env, &none);
        assert!(ffmpeg_input(&e, &ScreenTarget::Display(1), 30, false).is_err());
    }

    #[test]
    fn monitors_and_geometry() {
        let m = parse_monitors(MONITORS);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0],
            Monitor {
                name: "eDP-1".into(),
                primary: true,
                width: 1920,
                height: 1080,
                x: 0,
                y: 0
            }
        );
        assert!(!m[1].primary);
        assert_eq!(
            parse_geometry("1280/1x1024/1+-1280+0"),
            Some((1280, 1024, -1280, 0))
        );
        assert_eq!(
            parse_geometry("1280/1x1024/1+0+-1024"),
            Some((1280, 1024, 0, -1024))
        );
        assert_eq!(parse_geometry("800/1x600/1-10+5"), Some((800, 600, -10, 5)));
        assert_eq!(parse_geometry("garbage"), None);
        assert!(parse_monitors("").is_empty());
        assert!(parse_monitors("Monitors: 1\n 0: broken\n").is_empty());
        assert_eq!(find_window(TREE, "Slides"), Some("0x3a00003".into()));
        assert_eq!(find_window(TREE, "nothing"), None);
        assert_eq!(find_window("", "x"), None);
    }

    #[test]
    fn other_systems() {
        let var = |_: &str| None;
        let e = env("haiku", &no_run, &var);
        assert!(ffmpeg_input(&e, &ScreenTarget::Display(1), 30, true).is_err());
    }
}
