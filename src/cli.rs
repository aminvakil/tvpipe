//! Command line specs and parse functions (PLAN §4).

use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use argument::{Cli, Cmd, Error, Opt, Parser, Pos, ValueHint};

use crate::types::Encoder;
use crate::util::ratio::Ratio;

#[derive(Copy, Clone)]
enum Top {
    Verbose,
    Debug,
    Discover,
    Pair,
    Unpair,
    Play,
    TestPattern,
    DebugPipeline,
}

#[derive(Copy, Clone)]
enum DiscoverArg {
    Timeout,
    Json,
    Interface,
}

#[derive(Copy, Clone)]
enum PairArg {
    Target,
    Pin,
}

#[derive(Copy, Clone)]
enum UnpairArg {
    Target,
}

#[derive(Copy, Clone)]
enum PlayArg {
    To,
    Latency,
    Size,
    Fps,
    Bitrate,
    Encoder,
    NoAudio,
    Loop,
    Realtime,
    NoRealtime,
    RawVideo,
    RawAudio,
    Ffmpeg,
    Input,
}

#[derive(Copy, Clone)]
enum PipelineArg {
    Raw,
    RawVideo,
    RawAudio,
    Ffmpeg,
    Input,
}

static CLI: Cli<Top> = Cli::new("tvpipe")
    .version(env!("CARGO_PKG_VERSION"))
    .about("Low-latency mirroring of video and audio to AirPlay and Google Cast receivers.")
    .opts(&[
        Opt::new(Top::Verbose)
            .short('v')
            .long("verbose")
            .help("Print debug output"),
        Opt::new(Top::Debug)
            .long("debug")
            .help("Print protocol traces (very verbose)"),
    ])
    .commands(&[
        Cmd::new(Top::Discover, &DISCOVER),
        Cmd::new(Top::Pair, &PAIR),
        Cmd::new(Top::Unpair, &UNPAIR),
        Cmd::new(Top::Play, &PLAY),
        Cmd::new(Top::TestPattern, &TEST_PATTERN),
        Cmd::new(Top::DebugPipeline, &DEBUG_PIPELINE),
    ]);

static DISCOVER: Cli<DiscoverArg> = Cli::new("discover")
    .about("List AirPlay and Cast receivers on the network")
    .opts(&[
        Opt::new(DiscoverArg::Timeout)
            .long("timeout")
            .value("SECS")
            .default_value("3")
            .help("How long to browse"),
        Opt::new(DiscoverArg::Json)
            .long("json")
            .help("Print machine-readable JSON"),
        interface(DiscoverArg::Interface),
    ]);

static PAIR: Cli<PairArg> = Cli::new("pair")
    .about("Pair with an AirPlay receiver (the TV shows a PIN)")
    .opts(&[Opt::new(PairArg::Pin)
        .long("pin")
        .value("PIN")
        .help("The PIN shown on the TV; asked interactively if omitted")])
    .args(&[Pos::new(PairArg::Target, "NAME|ADDR").help("Receiver name or address")]);

static UNPAIR: Cli<UnpairArg> = Cli::new("unpair")
    .about("Forget the pairing with a receiver")
    .args(&[Pos::new(UnpairArg::Target, "NAME|ADDR").help("Receiver name or address")]);

const fn to<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("to")
        .value("NAME|ADDR")
        .help("Receiver to stream to (repeat for several)")
}

const fn interface<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("interface")
        .value("ADDR")
        .help("Also browse from this local address (repeatable)")
}

const fn ffmpeg_path<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("ffmpeg")
        .value("PATH")
        .value_hint(ValueHint::ExecutablePath)
        .help("Path to the ffmpeg binary")
}

const fn raw_video<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("raw-video")
        .value("WxH@FPS:PIXFMT")
        .help("Game mode: raw video in SLRW framing on stdin")
}

const fn raw_audio<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("raw-audio")
        .value("RATE:CH:FMT")
        .help("Game mode: raw audio in SLRW framing on stdin")
}

static PLAY_OPTS: &[Opt<PlayArg>] = &[
    to(PlayArg::To),
    Opt::new(PlayArg::Latency)
        .long("latency")
        .value("MS")
        .help("Target playout delay in milliseconds"),
    Opt::new(PlayArg::Size)
        .long("size")
        .value("WxH")
        .help("Output video size"),
    Opt::new(PlayArg::Fps)
        .long("fps")
        .value("N")
        .help("Output frame rate"),
    Opt::new(PlayArg::Bitrate)
        .long("bitrate")
        .value("KBPS")
        .help("Video bitrate in kbit/s"),
    Opt::new(PlayArg::Encoder)
        .long("encoder")
        .value("ENCODER")
        .default_value("auto")
        .possible_values(&[
            "auto",
            "builtin",
            "x264",
            "videotoolbox",
            "nvenc",
            "qsv",
            "vaapi",
            "mf",
        ])
        .help("Video encoder"),
    Opt::new(PlayArg::NoAudio)
        .long("no-audio")
        .help("Do not send audio"),
    Opt::new(PlayArg::Loop).long("loop").help("Loop the input"),
    Opt::new(PlayArg::Realtime)
        .long("realtime")
        .help("Pace the input in real time (default for files)"),
    Opt::new(PlayArg::NoRealtime)
        .long("no-realtime")
        .help("Do not pace the input (live sources)"),
    raw_video(PlayArg::RawVideo),
    raw_audio(PlayArg::RawAudio),
    ffmpeg_path(PlayArg::Ffmpeg),
];

static PLAY: Cli<PlayArg> = Cli::new("play")
    .about("Mirror a file, URL, capture device or raw game input")
    .usage("--to NAME|ADDR [OPTIONS] (INPUT | - | -- FFMPEG_INPUT_ARGS...)")
    .opts(PLAY_OPTS)
    .args(&[Pos::new(PlayArg::Input, "INPUT")
        .optional()
        .allow_hyphen()
        .value_hint(ValueHint::FilePath)
        .help("File or URL; '-' for SLRW raw input on stdin")])
    .after_help(
        "Arguments after '--' are passed to ffmpeg as input arguments, e.g.\n  \
         tvpipe play --to TV -- -f avfoundation -i 1:0",
    );

static TEST_PATTERN: Cli<PlayArg> = Cli::new("test-pattern")
    .about("Stream a test pattern with a burned-in wallclock for latency measurement")
    .opts(PLAY_OPTS);

static DEBUG_PIPELINE: Cli<PipelineArg> = Cli::new("debug-pipeline")
    .about("Run ffmpeg and the demuxer and print every frame")
    .usage("[OPTIONS] (INPUT | - | -- FFMPEG_INPUT_ARGS...)")
    .opts(&[
        Opt::new(PipelineArg::Raw)
            .long("raw")
            .help("Print frames without encoding a plan (ffmpeg defaults)"),
        raw_video(PipelineArg::RawVideo),
        raw_audio(PipelineArg::RawAudio),
        ffmpeg_path(PipelineArg::Ffmpeg),
    ])
    .args(&[Pos::new(PipelineArg::Input, "INPUT")
        .optional()
        .allow_hyphen()
        .value_hint(ValueHint::FilePath)
        .help("File or URL; '-' for SLRW raw input on stdin")]);

#[derive(Debug, Clone, Default)]
pub struct Global {
    pub verbose: bool,
    pub debug: bool,
}

/// Where the media comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSpec {
    /// A file path or URL given to ffmpeg's `-i`.
    Path(OsString),
    /// SLRW raw frames on stdin (`-`).
    Stdin,
    /// Raw ffmpeg input args after `--`.
    FfmpegArgs(Vec<OsString>),
    /// Generated test pattern.
    TestPattern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl FromStr for Size {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Size, Self::Err> {
        let (w, h) = s.split_once(['x', 'X']).ok_or("expected WxH")?;
        let width = w.parse().map_err(|_| "invalid width")?;
        let height = h.parse().map_err(|_| "invalid height")?;
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err("width and height must be positive and even");
        }
        Ok(Size { width, height })
    }
}

/// `--raw-video WxH@FPS:PIXFMT`, e.g. `1920x1080@60:rgba`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawVideoSpec {
    pub size: Size,
    pub fps: Ratio,
    pub pix_fmt: String,
}

impl FromStr for RawVideoSpec {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<RawVideoSpec, Self::Err> {
        let (size, rest) = s.split_once('@').ok_or("expected WxH@FPS:PIXFMT")?;
        let (fps, pix_fmt) = rest.split_once(':').ok_or("expected WxH@FPS:PIXFMT")?;
        if pix_fmt.is_empty()
            || !pix_fmt
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("invalid pixel format");
        }
        Ok(RawVideoSpec {
            size: size.parse()?,
            fps: fps.parse()?,
            pix_fmt: pix_fmt.to_owned(),
        })
    }
}

/// `--raw-audio RATE:CH:FMT`, e.g. `48000:2:s16le`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAudioSpec {
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_fmt: String,
}

impl FromStr for RawAudioSpec {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<RawAudioSpec, Self::Err> {
        let mut parts = s.split(':');
        let (Some(rate), Some(ch), Some(fmt), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("expected RATE:CH:FMT");
        };
        let sample_rate = rate.parse().map_err(|_| "invalid sample rate")?;
        let channels = ch.parse().map_err(|_| "invalid channel count")?;
        if sample_rate == 0 || channels == 0 {
            return Err("sample rate and channels must be positive");
        }
        if fmt.is_empty() || !fmt.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("invalid sample format");
        }
        Ok(RawAudioSpec {
            sample_rate,
            channels,
            sample_fmt: fmt.to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct PlayOptions {
    pub to: Vec<String>,
    pub latency_ms: Option<u32>,
    pub size: Option<Size>,
    pub fps: Option<Ratio>,
    pub bitrate_kbps: Option<u32>,
    pub encoder: Encoder,
    pub no_audio: bool,
    pub looped: bool,
    /// `None` means: decide from the input (files yes, live no).
    pub realtime: Option<bool>,
    pub raw_video: Option<RawVideoSpec>,
    pub raw_audio: Option<RawAudioSpec>,
    pub ffmpeg: Option<PathBuf>,
    pub input: InputSpec,
}

#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub raw: bool,
    pub raw_video: Option<RawVideoSpec>,
    pub raw_audio: Option<RawAudioSpec>,
    pub ffmpeg: Option<PathBuf>,
    pub input: InputSpec,
}

#[derive(Debug, Clone)]
pub enum Command {
    Discover {
        timeout: Duration,
        json: bool,
        interfaces: Vec<std::net::Ipv4Addr>,
    },
    Pair {
        target: String,
        pin: Option<String>,
    },
    Unpair {
        target: String,
    },
    Play(PlayOptions),
    TestPattern(PlayOptions),
    DebugPipeline(PipelineOptions),
}

/// Parses `std::env::args`; prints help/errors and exits on failure.
pub fn parse_env() -> (Global, Command) {
    CLI.run(parse_top)
}

#[cfg(test)]
fn parse_from(args: &[&str]) -> Result<(Global, Command), Error> {
    parse_top(CLI.parser_from_args(args.iter().copied()))
}

fn parse_top(mut p: Parser<Top>) -> Result<(Global, Command), Error> {
    let mut g = Global::default();
    while let Some(arg) = p.param()? {
        match arg {
            Top::Verbose => g.verbose = true,
            Top::Debug => g.debug = true,
            Top::Discover => return Ok((g, parse_discover(p.subcommand(&DISCOVER))?)),
            Top::Pair => return Ok((g, parse_pair(p.subcommand(&PAIR))?)),
            Top::Unpair => return Ok((g, parse_unpair(p.subcommand(&UNPAIR))?)),
            Top::Play => return Ok((g, Command::Play(parse_play(p.subcommand(&PLAY), false)?))),
            Top::TestPattern => {
                return Ok((
                    g,
                    Command::TestPattern(parse_play(p.subcommand(&TEST_PATTERN), true)?),
                ))
            }
            Top::DebugPipeline => return Ok((g, parse_pipeline(p.subcommand(&DEBUG_PIPELINE))?)),
        }
    }
    Err(p.help())
}

fn parse_discover(mut p: Parser<DiscoverArg>) -> Result<Command, Error> {
    let mut timeout = 3.0f64;
    let mut json = false;
    let mut interfaces = Vec::new();
    while let Some(arg) = p.param()? {
        match arg {
            DiscoverArg::Timeout => timeout = p.value()?,
            DiscoverArg::Json => json = true,
            DiscoverArg::Interface => interfaces.push(p.value()?),
        }
    }
    if !(timeout.is_finite() && timeout > 0.0 && timeout <= 3600.0) {
        return Err(p.error("--timeout must be between 0 and 3600 seconds"));
    }
    Ok(Command::Discover {
        timeout: Duration::from_secs_f64(timeout),
        json,
        interfaces,
    })
}

fn parse_pair(mut p: Parser<PairArg>) -> Result<Command, Error> {
    let mut target = String::new();
    let mut pin = None;
    while let Some(arg) = p.param()? {
        match arg {
            PairArg::Target => target = p.value()?,
            PairArg::Pin => pin = Some(p.value()?),
        }
    }
    Ok(Command::Pair { target, pin })
}

fn parse_unpair(mut p: Parser<UnpairArg>) -> Result<Command, Error> {
    let mut target = String::new();
    while let Some(arg) = p.param()? {
        match arg {
            UnpairArg::Target => target = p.value()?,
        }
    }
    Ok(Command::Unpair { target })
}

/// Collects everything after a literal `--` as ffmpeg input args.
fn take_after_double_dash<A: Copy + 'static>(p: &mut Parser<A>) -> Option<Vec<OsString>> {
    if p.peek_raw_arg().is_some_and(|a| a == "--") {
        p.raw_arg();
        let mut rest = Vec::new();
        while let Some(a) = p.raw_arg() {
            rest.push(a);
        }
        Some(rest)
    } else {
        None
    }
}

fn input_from(raw: OsString) -> InputSpec {
    if raw == "-" {
        InputSpec::Stdin
    } else {
        InputSpec::Path(raw)
    }
}

fn parse_play(mut p: Parser<PlayArg>, test_pattern: bool) -> Result<PlayOptions, Error> {
    let mut o = PlayOptions {
        to: Vec::new(),
        latency_ms: None,
        size: None,
        fps: None,
        bitrate_kbps: None,
        encoder: Encoder::Auto,
        no_audio: false,
        looped: false,
        realtime: None,
        raw_video: None,
        raw_audio: None,
        ffmpeg: None,
        input: InputSpec::TestPattern,
    };
    let mut input = None;
    loop {
        if !test_pattern {
            if let Some(args) = take_after_double_dash(&mut p) {
                if input.is_some() {
                    return Err(p.error("cannot combine INPUT with '-- FFMPEG_INPUT_ARGS'"));
                }
                if args.is_empty() {
                    return Err(p.error("expected ffmpeg input arguments after '--'"));
                }
                input = Some(InputSpec::FfmpegArgs(args));
                break;
            }
        }
        let Some(arg) = p.param()? else { break };
        match arg {
            PlayArg::To => o.to.push(p.value()?),
            PlayArg::Latency => o.latency_ms = Some(p.value()?),
            PlayArg::Size => o.size = Some(p.value()?),
            PlayArg::Fps => o.fps = Some(p.value()?),
            PlayArg::Bitrate => o.bitrate_kbps = Some(p.value()?),
            PlayArg::Encoder => o.encoder = parse_encoder(&p.value::<String>()?),
            PlayArg::NoAudio => o.no_audio = true,
            PlayArg::Loop => o.looped = true,
            PlayArg::Realtime => o.realtime = Some(true),
            PlayArg::NoRealtime => o.realtime = Some(false),
            PlayArg::RawVideo => o.raw_video = Some(p.value()?),
            PlayArg::RawAudio => o.raw_audio = Some(p.value()?),
            PlayArg::Ffmpeg => o.ffmpeg = Some(p.raw_value()?.into()),
            PlayArg::Input => input = Some(input_from(p.raw_value()?)),
        }
    }
    if o.to.is_empty() {
        return Err(p.error("at least one --to NAME|ADDR is required"));
    }
    if !test_pattern {
        o.input = input.ok_or_else(|| p.error("missing INPUT (or '-- FFMPEG_INPUT_ARGS')"))?;
    }
    let raw = o.raw_video.is_some() || o.raw_audio.is_some();
    if raw && o.input != InputSpec::Stdin {
        return Err(p.error("--raw-video/--raw-audio require '-' as INPUT"));
    }
    if o.input == InputSpec::Stdin && !raw {
        return Err(p.error("'-' as INPUT requires --raw-video and/or --raw-audio"));
    }
    Ok(o)
}

fn parse_pipeline(mut p: Parser<PipelineArg>) -> Result<Command, Error> {
    let mut o = PipelineOptions {
        raw: false,
        raw_video: None,
        raw_audio: None,
        ffmpeg: None,
        input: InputSpec::TestPattern,
    };
    let mut input = None;
    loop {
        if let Some(args) = take_after_double_dash(&mut p) {
            if input.is_some() || args.is_empty() {
                return Err(p.error("expected either INPUT or '-- FFMPEG_INPUT_ARGS...'"));
            }
            input = Some(InputSpec::FfmpegArgs(args));
            break;
        }
        let Some(arg) = p.param()? else { break };
        match arg {
            PipelineArg::Raw => o.raw = true,
            PipelineArg::RawVideo => o.raw_video = Some(p.value()?),
            PipelineArg::RawAudio => o.raw_audio = Some(p.value()?),
            PipelineArg::Ffmpeg => o.ffmpeg = Some(p.raw_value()?.into()),
            PipelineArg::Input => input = Some(input_from(p.raw_value()?)),
        }
    }
    // No input means: test pattern.
    o.input = input.unwrap_or(InputSpec::TestPattern);
    Ok(Command::DebugPipeline(o))
}

fn parse_encoder(s: &str) -> Encoder {
    match s {
        "builtin" => Encoder::Builtin,
        "x264" => Encoder::X264,
        "videotoolbox" => Encoder::VideoToolbox,
        "nvenc" => Encoder::Nvenc,
        "qsv" => Encoder::Qsv,
        "vaapi" => Encoder::Vaapi,
        "mf" => Encoder::MediaFoundation,
        _ => Encoder::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_with_ffmpeg_args() {
        let (_, cmd) = parse_from(&[
            "-v",
            "play",
            "--to",
            "Living Room",
            "--to",
            "10.0.0.5",
            "--fps",
            "30",
            "--",
            "-f",
            "avfoundation",
            "-i",
            "1:0",
        ])
        .unwrap();
        let Command::Play(o) = cmd else { panic!() };
        assert_eq!(o.to, ["Living Room", "10.0.0.5"]);
        assert_eq!(o.fps, Some(Ratio::integer(30)));
        assert_eq!(
            o.input,
            InputSpec::FfmpegArgs(
                ["-f", "avfoundation", "-i", "1:0"]
                    .map(OsString::from)
                    .to_vec()
            )
        );
    }

    #[test]
    fn play_raw_stdin() {
        let (_, cmd) = parse_from(&[
            "play",
            "--to",
            "tv",
            "--raw-video",
            "1920x1080@60:rgba",
            "--raw-audio",
            "48000:2:s16le",
            "-",
        ])
        .unwrap();
        let Command::Play(o) = cmd else { panic!() };
        assert_eq!(o.input, InputSpec::Stdin);
        assert_eq!(
            o.raw_video.unwrap().size,
            Size {
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(o.raw_audio.unwrap().channels, 2);
    }

    #[test]
    fn builtin_encoder() {
        let (_, cmd) = parse_from(&["test-pattern", "--to", "tv", "--encoder", "builtin"]).unwrap();
        let Command::TestPattern(o) = cmd else {
            panic!()
        };
        assert_eq!(o.encoder, Encoder::Builtin);
        assert_eq!(o.input, InputSpec::TestPattern);
    }

    #[test]
    fn play_requires_to() {
        assert!(parse_from(&["play", "movie.mkv"]).is_err());
        assert!(parse_from(&["play", "--to", "tv", "-"]).is_err());
    }

    #[test]
    fn discover_defaults() {
        let (_, cmd) = parse_from(&["discover"]).unwrap();
        let Command::Discover { timeout, json, .. } = cmd else {
            panic!()
        };
        assert_eq!(timeout, Duration::from_secs(3));
        assert!(!json);
    }

    #[test]
    fn specs() {
        assert!("1920x1080@30000/1001:yuv420p"
            .parse::<RawVideoSpec>()
            .is_ok());
        assert!("1920x1081@30:rgba".parse::<RawVideoSpec>().is_err());
        assert!("48000:2".parse::<RawAudioSpec>().is_err());
    }
}
