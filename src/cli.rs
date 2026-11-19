//! Command line specs and parse functions (PLAN §4).

use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use argument::{Cli, Cmd, Error, Opt, Parser, Pos, ValueHint};

use crate::net::tunnel::Provider;
use crate::types::Encoder;
use crate::util::ratio::Ratio;

#[derive(Copy, Clone)]
enum Top {
    Verbose,
    Debug,
    Machine,
    Discover,
    Pair,
    Unpair,
    Play,
    TestPattern,
    Preview,
    Share,
    Screen,
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
    Reconnect,
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
    Port,
    Open,
    WaitForViewer,
    NewToken,
    Tunnel,
    TunnelTtl,
    Input,
}

#[derive(Copy, Clone)]
enum ShareArg {
    To,
    Reconnect,
    Latency,
    Size,
    Fps,
    Bitrate,
    NoAudio,
    Port,
    Open,
    WaitForViewer,
    NewToken,
    Tunnel,
    TunnelTtl,
    Yes,
    Profile,
    Visible,
    Reload,
    Browser,
    Url,
}

#[derive(Copy, Clone)]
enum ScreenArg {
    To,
    Reconnect,
    Latency,
    Size,
    Fps,
    Bitrate,
    NoAudio,
    Display,
    Window,
    Pick,
    Engine,
    Browser,
    Ffmpeg,
    Port,
    Open,
    WaitForViewer,
    NewToken,
    Tunnel,
    TunnelTtl,
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
        Opt::new(Top::Machine)
            .long("machine")
            .help("For programs: JSON events on stdout, commands on stdin (docs/machine.md)"),
    ])
    .commands(&[
        Cmd::new(Top::Discover, &DISCOVER),
        Cmd::new(Top::Pair, &PAIR),
        Cmd::new(Top::Unpair, &UNPAIR),
        Cmd::new(Top::Play, &PLAY),
        Cmd::new(Top::TestPattern, &TEST_PATTERN),
        Cmd::new(Top::Preview, &PREVIEW),
        Cmd::new(Top::Share, &SHARE),
        Cmd::new(Top::Screen, &SCREEN),
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

/// How long a receiver that dropped out is reconnected by default.
pub const DEFAULT_RECONNECT: Duration = Duration::from_secs(120);

const fn reconnect<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("reconnect")
        .value("SECS")
        .default_value("120")
        .help("Reconnect a receiver that drops out for this long (by name: found again, e.g. at a new address; 0: off)")
}

/// `--reconnect SECS`.
fn reconnect_secs<A: Copy + 'static>(p: &mut Parser<A>) -> Result<Duration, Error> {
    let secs: f64 = p.value()?;
    if !(secs.is_finite() && (0.0..=86_400.0).contains(&secs)) {
        return Err(p.error("--reconnect takes seconds (0 to 86400)"));
    }
    Ok(Duration::from_secs_f64(secs))
}

// The `--to browser` player page's options, the same in every subcommand
// that streams (`play`, `test-pattern`, `preview`, `share`).

const fn player_port<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("port")
        .value("PORT")
        .help("Browser player page port on 127.0.0.1 (--to browser; default 8765, 0: any)")
}

const fn player_open<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("open")
        .help("Open the browser player page in the default browser (--to browser)")
}

const fn wait_for_viewer<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("wait-for-viewer")
        .help("Start streaming once a browser tab is watching (--to browser)")
}

const fn new_token<A>(id: A) -> Opt<A> {
    Opt::new(id)
        .long("new-token")
        .help("Change the browser player page's URL token (--to browser; old URLs stop working)")
}

/// `--tunnel[=PROVIDER]` (docs/tunnels.md).
const fn tunnel<A>(id: A, help: &'static str) -> Opt<A> {
    Opt::new(id)
        .long("tunnel")
        .optional_value("PROVIDER")
        .help(help)
}

/// The provider `--tunnel[=PROVIDER]` names (none: the default).
fn tunnel_value<A: Copy + 'static>(p: &mut Parser<A>) -> Result<Option<Provider>, Error> {
    match p.optional_value::<String>()? {
        None => Ok(Some(Provider::default())),
        Some(v) => match Provider::parse(&v) {
            Some(provider) => Ok(Some(provider)),
            None => Err(p.error("--tunnel=PROVIDER: cloudflare (the default) or opentunnel")),
        },
    }
}

const fn tunnel_ttl<A>(id: A, help: &'static str) -> Opt<A> {
    Opt::new(id).long("tunnel-ttl").value("MIN").help(help)
}

/// `--tunnel opentunnel` makes `opentunnel` the input: say how to write it.
fn tunnel_typo<A: Copy + 'static>(
    p: &Parser<A>,
    tunnel: Option<Provider>,
    arg: Option<&str>,
) -> Result<(), Error> {
    match arg.and_then(Provider::parse) {
        Some(provider) if tunnel.is_some() => Err(p.error(format!(
            "write --tunnel={} (the provider is attached with '=')",
            provider.name()
        ))),
        _ => Ok(()),
    }
}

/// The share page's public link lets people in for this long by default.
pub const SHARE_TUNNEL_TTL: Duration = Duration::from_secs(60 * 60);

/// `--tunnel-ttl MIN` (0: no expiry).
fn tunnel_ttl_value<A: Copy + 'static>(p: &mut Parser<A>) -> Result<Option<Duration>, Error> {
    let min: f64 = p.value()?;
    if !(min.is_finite() && (0.0..=7.0 * 24.0 * 60.0).contains(&min)) {
        return Err(p.error("--tunnel-ttl takes minutes (0 to 10080; 0: no expiry)"));
    }
    Ok((min > 0.0).then(|| Duration::from_secs_f64(min * 60.0)))
}

/// Whether `--to browser` is among the receivers.
fn has_browser_target(to: &[String]) -> bool {
    to.iter().any(|t| t.eq_ignore_ascii_case(BROWSER_TARGET))
}

const PLAYER_ONLY: &str =
    "--port, --open, --wait-for-viewer, --new-token and --tunnel are for --to browser";

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

/// `play`'s options after the given ones (`preview` has no `--to`).
macro_rules! play_opts {
    ($($first:expr),*) => {
        &[
            $($first,)*
            Opt::new(PlayArg::Latency)
                .long("latency")
                .value("MS")
                .help("Playout delay in ms, the same on every receiver (default: Cast 300, AirPlay automatic ~100-170, browser 100)"),
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
            player_port(PlayArg::Port),
            player_open(PlayArg::Open),
            wait_for_viewer(PlayArg::WaitForViewer),
            new_token(PlayArg::NewToken),
            tunnel(
                PlayArg::Tunnel,
                "Also put the browser player page on a public https URL: anyone with the link can watch (PROVIDER: cloudflare, the default, or opentunnel)",
            ),
            tunnel_ttl(
                PlayArg::TunnelTtl,
                "With --tunnel: let new viewers in for MIN minutes only (default: no limit)",
            ),
        ]
    };
}

static PLAY_OPTS: &[Opt<PlayArg>] = play_opts!(to(PlayArg::To), reconnect(PlayArg::Reconnect));

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
    .opts(PLAY_OPTS)
    .after_help(
        "The pattern (UTC clock in ms, frame counter, a beep and a flash every second) is \
         generated and encoded in-process; ffmpeg is not needed. Naming an ffmpeg encoder \
         (e.g. --encoder x264) streams ffmpeg's testsrc2 instead.",
    );

static PREVIEW: Cli<PlayArg> = Cli::new("preview")
    .about("Play a file, URL, capture device, raw game input or the test pattern in a browser tab")
    .usage("[OPTIONS] [INPUT | - | -- FFMPEG_INPUT_ARGS...]")
    .opts(play_opts!())
    .args(&[Pos::new(PlayArg::Input, "INPUT")
        .optional()
        .allow_hyphen()
        .value_hint(ValueHint::FilePath)
        .help("File or URL; '-' for SLRW raw input on stdin; none: the test pattern")])
    .after_help(
        "Serves a player page on 127.0.0.1 and prints its URL (--open opens it); the same\n\
         as 'play --to browser'. The tab also works as an OBS browser source, or shared\n\
         into a video call (docs/meet.md).",
    );

static SHARE: Cli<ShareArg> = Cli::new("share")
    .about("Mirror a browser tab, window or screen, or a web page (URL) in a browser of tvpipe's own")
    .usage("[URL] --to NAME|ADDR [OPTIONS]")
    .opts(&[
        to(ShareArg::To),
        reconnect(ShareArg::Reconnect),
        Opt::new(ShareArg::Latency)
            .long("latency")
            .value("MS")
            .help("Playout delay in ms, the same on every receiver"),
        Opt::new(ShareArg::Size)
            .long("size")
            .value("WxH")
            .help("Output video size (default: the capture, or with URL the receivers' screen shape; at most 1920x1080)"),
        Opt::new(ShareArg::Fps)
            .long("fps")
            .value("N")
            .help("Output frame rate (default 30)"),
        Opt::new(ShareArg::Bitrate)
            .long("bitrate")
            .value("KBPS")
            .help("Video bitrate in kbit/s"),
        Opt::new(ShareArg::NoAudio)
            .long("no-audio")
            .help("Do not send sound"),
        Opt::new(ShareArg::Open)
            .long("open")
            .help("Open the share page (without URL) and the browser player page (--to browser) in the default browser"),
        player_port(ShareArg::Port),
        wait_for_viewer(ShareArg::WaitForViewer),
        new_token(ShareArg::NewToken),
        tunnel(
            ShareArg::Tunnel,
            "Also put the share page on a public https URL: someone with the link can ask to share their screen to the receivers, you confirm each (PROVIDER: cloudflare, the default, or opentunnel)",
        ),
        tunnel_ttl(
            ShareArg::TunnelTtl,
            "With --tunnel: the link lets people in for MIN minutes (default 60; 0: no limit)",
        ),
        Opt::new(ShareArg::Yes)
            .long("yes")
            .short('y')
            .help("With --tunnel: show a remote share without asking first"),
        Opt::new(ShareArg::Visible)
            .long("visible")
            .help("With URL: show the browser window instead of running headless (e.g. to log in)"),
        Opt::new(ShareArg::Profile)
            .long("profile")
            .value("DIR")
            .value_hint(ValueHint::DirPath)
            .help("With URL: keep the browser's profile (logins, cookies) in DIR (default: a temporary one)"),
        Opt::new(ShareArg::Reload)
            .long("reload")
            .value("MIN")
            .help("With URL: reload the page every MIN minutes"),
        Opt::new(ShareArg::Browser)
            .long("browser")
            .value("PATH")
            .value_hint(ValueHint::ExecutablePath)
            .help("With URL: the browser: a Chromium-based one or Firefox, a path or 'firefox' (default: $TVPIPE_BROWSER, then Chrome, Edge, Chromium, Brave, Firefox)"),
    ])
    .args(&[Pos::new(ShareArg::Url, "URL")
        .optional()
        .value_hint(ValueHint::Url)
        .help("A page to show instead of picking a tab: http(s)://..., file://... or a local file")])
    .after_help(
        "Without URL: open the printed http://127.0.0.1 page in a browser with WebCodecs\n\
         (Chrome, Edge, Safari 16.4+, Firefox 130+) and press Share. Closing the page or\n\
         stopping the share ends the session.\n\
         \n\
         With URL: tvpipe starts a Chromium-based browser (or Firefox) with a profile of its\n\
         own (never yours), sized to the stream, opens URL and shares that tab with its\n\
         sound. It is started again if it crashes; Ctrl-C or 'q' stops. Sites that refuse\n\
         automated browsers (e.g. Google Meet) do not work.",
    );

static SCREEN: Cli<ScreenArg> = Cli::new("screen")
    .about("Mirror a screen or a window, with the computer's sound")
    .usage("--to NAME|ADDR [--display N | --window TITLE | --pick] [OPTIONS]")
    .opts(&[
        to(ScreenArg::To),
        reconnect(ScreenArg::Reconnect),
        Opt::new(ScreenArg::Latency)
            .long("latency")
            .value("MS")
            .help("Playout delay in ms, the same on every receiver"),
        Opt::new(ScreenArg::Size)
            .long("size")
            .value("WxH")
            .help("Output video size (default: the screen's, within the receivers' limits)"),
        Opt::new(ScreenArg::Fps)
            .long("fps")
            .value("N")
            .help("Output frame rate (default 30)"),
        Opt::new(ScreenArg::Bitrate)
            .long("bitrate")
            .value("KBPS")
            .help("Video bitrate in kbit/s"),
        Opt::new(ScreenArg::NoAudio)
            .long("no-audio")
            .help("Do not send sound"),
        Opt::new(ScreenArg::Display)
            .long("display")
            .value("N|NAME")
            .help("The screen: 1 (the main one, the default), 2, ...; or a part of its name in the browser's picker"),
        Opt::new(ScreenArg::Window)
            .long("window")
            .value("TITLE")
            .help("Share the window whose title contains TITLE (case matters)"),
        Opt::new(ScreenArg::Pick)
            .long("pick")
            .help("Choose the screen or window in the browser's picker"),
        Opt::new(ScreenArg::Engine)
            .long("engine")
            .value("browser|ffmpeg")
            .help("Capture with a Chromium-based browser (default when one is installed) or with ffmpeg"),
        Opt::new(ScreenArg::Browser)
            .long("browser")
            .value("PATH")
            .value_hint(ValueHint::ExecutablePath)
            .help("The Chromium-based browser (default: $TVPIPE_BROWSER, then Chrome, Edge, Chromium, Brave)"),
        ffmpeg_path(ScreenArg::Ffmpeg),
        player_port(ScreenArg::Port),
        player_open(ScreenArg::Open),
        wait_for_viewer(ScreenArg::WaitForViewer),
        new_token(ScreenArg::NewToken),
        tunnel(
            ScreenArg::Tunnel,
            "Also put the browser player page on a public https URL: anyone with the link can watch (PROVIDER: cloudflare, the default, or opentunnel)",
        ),
        tunnel_ttl(
            ScreenArg::TunnelTtl,
            "With --tunnel: let new viewers in for MIN minutes only (default: no limit)",
        ),
    ])
    .after_help(
        "tvpipe starts a Chromium-based browser of its own (a separate profile, a small\n\
         window it minimizes) whose page captures the screen or window with the computer's\n\
         sound and encodes it. The browser needs the system's permission to record the\n\
         screen (macOS: Privacy & Security > Screen & System Audio Recording). Stop with\n\
         Ctrl-C, 'q' or the browser's \"Stop sharing\". --engine ffmpeg captures with\n\
         ffmpeg instead: no sound on macOS and Windows, no --window on macOS.",
    );

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
    /// `--machine`: the machine interface (`crate::machine`).
    pub machine: bool,
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
    /// `--reconnect`: how long a lost receiver is reconnected (zero: off).
    pub reconnect: Duration,
    /// For `--to browser` receivers.
    pub browser: BrowserOptions,
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

/// The `--to` that names the browser receiver (`--to browser`).
pub const BROWSER_TARGET: &str = "browser";

/// Options of `--to browser` receivers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserOptions {
    /// Port of the player page (`None`: the fixed default, 0: any free one).
    pub port: Option<u16>,
    pub open: bool,
    pub wait_for_viewer: bool,
    /// Rotate the page's persistent token.
    pub new_token: bool,
    /// Also serve the page on a public URL (`--tunnel[=PROVIDER]`).
    pub tunnel: Option<Provider>,
    /// `--tunnel-ttl` (`None`: no expiry).
    pub tunnel_ttl: Option<Duration>,
}

/// `tvpipe share [URL]`.
#[derive(Debug, Clone, Default)]
pub struct ShareOptions {
    pub to: Vec<String>,
    /// `--reconnect` (zero, the `Default`: off).
    pub reconnect: Duration,
    pub latency_ms: Option<u32>,
    pub size: Option<Size>,
    pub fps: Option<Ratio>,
    pub bitrate_kbps: Option<u32>,
    pub no_audio: bool,
    /// Open the share page (without URL) and the player page.
    pub open: bool,
    /// For `--to browser` receivers (`open` is the field above).
    pub player: BrowserOptions,
    /// Also serve the share page on a public URL (`--tunnel[=PROVIDER]`;
    /// not with URL).
    pub tunnel: Option<Provider>,
    /// How long the public link lets people in (`--tunnel-ttl`, default
    /// [`SHARE_TUNNEL_TTL`]; `None`: no expiry).
    pub tunnel_ttl: Option<Duration>,
    /// `--yes`: show a remote share without asking.
    pub yes: bool,
    /// With URL: tvpipe's own browser shows it.
    pub page: Option<PageOptions>,
}

/// `tvpipe screen`.
#[derive(Debug, Clone, Default)]
pub struct ScreenOptions {
    /// The streaming options (`page` is `None`; `open` is the player's).
    pub share: ShareOptions,
    pub target: ScreenTarget,
    /// `None`: the browser when one is installed, else ffmpeg.
    pub engine: Option<ScreenEngine>,
    pub browser: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

/// What `tvpipe screen` shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenTarget {
    /// `--display N` (1 is the main screen, the default).
    Display(u32),
    /// `--display NAME`: a part of the screen's name in the picker.
    DisplayNamed(String),
    /// `--window TITLE`: a part of the window's title.
    Window(String),
    /// `--pick`: the user chooses in the browser's picker.
    Pick,
}

impl Default for ScreenTarget {
    fn default() -> Self {
        ScreenTarget::Display(1)
    }
}

/// How `tvpipe screen` captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenEngine {
    Browser,
    Ffmpeg,
}

/// `share URL`: the page and tvpipe's browser.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageOptions {
    pub url: String,
    pub profile: Option<PathBuf>,
    pub visible: bool,
    pub reload: Option<Duration>,
    pub browser: Option<PathBuf>,
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
    Share(ShareOptions),
    Screen(ScreenOptions),
    DebugPipeline(PipelineOptions),
}

impl Command {
    /// The subcommand's name (`preview` is `play`).
    pub fn name(&self) -> &'static str {
        match self {
            Command::Discover { .. } => "discover",
            Command::Pair { .. } => "pair",
            Command::Unpair { .. } => "unpair",
            Command::Play(_) => "play",
            Command::TestPattern(_) => "test-pattern",
            Command::Share(_) => "share",
            Command::Screen(_) => "screen",
            Command::DebugPipeline(_) => "debug-pipeline",
        }
    }
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
            Top::Machine => g.machine = true,
            Top::Discover => return Ok((g, parse_discover(p.subcommand(&DISCOVER))?)),
            Top::Pair => return Ok((g, parse_pair(p.subcommand(&PAIR))?)),
            Top::Unpair => return Ok((g, parse_unpair(p.subcommand(&UNPAIR))?)),
            Top::Play => {
                return Ok((
                    g,
                    Command::Play(parse_play(p.subcommand(&PLAY), Mode::Play)?),
                ))
            }
            Top::TestPattern => {
                return Ok((
                    g,
                    Command::TestPattern(parse_play(
                        p.subcommand(&TEST_PATTERN),
                        Mode::TestPattern,
                    )?),
                ))
            }
            Top::Preview => {
                return Ok((
                    g,
                    Command::Play(parse_play(p.subcommand(&PREVIEW), Mode::Preview)?),
                ))
            }
            Top::Share => return Ok((g, parse_share(p.subcommand(&SHARE))?)),
            Top::Screen => return Ok((g, parse_screen(p.subcommand(&SCREEN))?)),
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

/// Which command `parse_play` parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Play,
    TestPattern,
    /// `play --to browser`; no INPUT: the test pattern.
    Preview,
}

fn parse_play(mut p: Parser<PlayArg>, mode: Mode) -> Result<PlayOptions, Error> {
    let test_pattern = mode == Mode::TestPattern;
    let mut o = PlayOptions {
        to: Vec::new(),
        reconnect: DEFAULT_RECONNECT,
        browser: BrowserOptions::default(),
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
            PlayArg::Reconnect => o.reconnect = reconnect_secs(&mut p)?,
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
            PlayArg::Port => o.browser.port = Some(p.value()?),
            PlayArg::Open => o.browser.open = true,
            PlayArg::WaitForViewer => o.browser.wait_for_viewer = true,
            PlayArg::NewToken => o.browser.new_token = true,
            PlayArg::Tunnel => o.browser.tunnel = tunnel_value(&mut p)?,
            PlayArg::TunnelTtl => o.browser.tunnel_ttl = tunnel_ttl_value(&mut p)?,
            PlayArg::Input => input = Some(input_from(p.raw_value()?)),
        }
    }
    if mode == Mode::Preview {
        o.to = vec![BROWSER_TARGET.to_owned()];
    }
    if o.browser.tunnel_ttl.is_some() && o.browser.tunnel.is_none() {
        return Err(p.error("--tunnel-ttl is for --tunnel"));
    }
    if let Some(InputSpec::Path(i)) = &input {
        tunnel_typo(&p, o.browser.tunnel, i.to_str())?;
    }
    if o.to.is_empty() {
        return Err(p.error("at least one --to NAME|ADDR is required"));
    }
    if !has_browser_target(&o.to) && o.browser != BrowserOptions::default() {
        return Err(p.error(PLAYER_ONLY));
    }
    match mode {
        Mode::Play => {
            o.input = input.ok_or_else(|| p.error("missing INPUT (or '-- FFMPEG_INPUT_ARGS')"))?;
        }
        Mode::Preview => o.input = input.unwrap_or(InputSpec::TestPattern),
        Mode::TestPattern => {}
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

fn parse_share(mut p: Parser<ShareArg>) -> Result<Command, Error> {
    let mut o = ShareOptions {
        reconnect: DEFAULT_RECONNECT,
        ..ShareOptions::default()
    };
    let mut page = PageOptions::default();
    let mut url = None;
    let mut ttl = None;
    while let Some(arg) = p.param()? {
        match arg {
            ShareArg::To => o.to.push(p.value()?),
            ShareArg::Reconnect => o.reconnect = reconnect_secs(&mut p)?,
            ShareArg::Latency => o.latency_ms = Some(p.value()?),
            ShareArg::Size => o.size = Some(p.value()?),
            ShareArg::Fps => o.fps = Some(p.value()?),
            ShareArg::Bitrate => o.bitrate_kbps = Some(p.value()?),
            ShareArg::NoAudio => o.no_audio = true,
            ShareArg::Open => o.open = true,
            ShareArg::Port => o.player.port = Some(p.value()?),
            ShareArg::WaitForViewer => o.player.wait_for_viewer = true,
            ShareArg::NewToken => o.player.new_token = true,
            ShareArg::Tunnel => o.tunnel = tunnel_value(&mut p)?,
            ShareArg::TunnelTtl => ttl = Some(tunnel_ttl_value(&mut p)?),
            ShareArg::Yes => o.yes = true,
            ShareArg::Profile => page.profile = Some(p.value()?),
            ShareArg::Visible => page.visible = true,
            ShareArg::Reload => {
                let min: f64 = p.value()?;
                if !(min.is_finite() && min > 0.0 && min <= 7.0 * 24.0 * 60.0) {
                    return Err(p.error("--reload takes minutes (> 0)"));
                }
                page.reload = Some(Duration::from_secs_f64(min * 60.0));
            }
            ShareArg::Browser => page.browser = Some(p.value()?),
            ShareArg::Url => url = Some(p.value::<String>()?),
        }
    }
    if o.to.is_empty() {
        return Err(p.error("at least one --to NAME|ADDR is required"));
    }
    let browser = has_browser_target(&o.to);
    if !browser && o.player != BrowserOptions::default() {
        return Err(p.error(PLAYER_ONLY));
    }
    tunnel_typo(&p, o.tunnel, url.as_deref())?;
    if (ttl.is_some() || o.yes) && o.tunnel.is_none() {
        return Err(p.error("--tunnel-ttl and --yes are for --tunnel"));
    }
    o.tunnel_ttl = ttl.unwrap_or(Some(SHARE_TUNNEL_TTL));
    match url {
        Some(_) if o.tunnel.is_some() => {
            return Err(p.error(
                "--tunnel serves the share page (without URL) to others; with URL tvpipe's own browser shares",
            ));
        }
        Some(url) => {
            if o.open && !browser {
                return Err(p.error(
                    "--open opens the share page (without URL) or the --to browser player page",
                ));
            }
            page.url = url;
            o.page = Some(page);
        }
        None if page != PageOptions::default() => {
            return Err(p.error(
                "--visible, --profile, --reload and --browser are for 'share URL' (tvpipe's own browser)",
            ));
        }
        None => {}
    }
    Ok(Command::Share(o))
}

fn parse_screen(mut p: Parser<ScreenArg>) -> Result<Command, Error> {
    let mut o = ScreenOptions {
        share: ShareOptions {
            reconnect: DEFAULT_RECONNECT,
            ..ShareOptions::default()
        },
        ..ScreenOptions::default()
    };
    let mut targets = 0;
    while let Some(arg) = p.param()? {
        let s = &mut o.share;
        match arg {
            ScreenArg::To => s.to.push(p.value()?),
            ScreenArg::Reconnect => s.reconnect = reconnect_secs(&mut p)?,
            ScreenArg::Latency => s.latency_ms = Some(p.value()?),
            ScreenArg::Size => s.size = Some(p.value()?),
            ScreenArg::Fps => s.fps = Some(p.value()?),
            ScreenArg::Bitrate => s.bitrate_kbps = Some(p.value()?),
            ScreenArg::NoAudio => s.no_audio = true,
            ScreenArg::Open => s.open = true,
            ScreenArg::Port => s.player.port = Some(p.value()?),
            ScreenArg::WaitForViewer => s.player.wait_for_viewer = true,
            ScreenArg::NewToken => s.player.new_token = true,
            ScreenArg::Tunnel => s.player.tunnel = tunnel_value(&mut p)?,
            ScreenArg::TunnelTtl => s.player.tunnel_ttl = tunnel_ttl_value(&mut p)?,
            ScreenArg::Display => {
                targets += 1;
                let v: String = p.value()?;
                o.target = match v.trim().parse::<u32>() {
                    Ok(0) => return Err(p.error("--display counts from 1 (the main screen)")),
                    Ok(n) => ScreenTarget::Display(n),
                    Err(_) if v.trim().is_empty() => {
                        return Err(p.error("--display takes a number or a name"))
                    }
                    Err(_) => ScreenTarget::DisplayNamed(v),
                };
            }
            ScreenArg::Window => {
                targets += 1;
                let v: String = p.value()?;
                if v.trim().is_empty() {
                    return Err(p.error("--window takes (a part of) the window's title"));
                }
                o.target = ScreenTarget::Window(v);
            }
            ScreenArg::Pick => {
                targets += 1;
                o.target = ScreenTarget::Pick;
            }
            ScreenArg::Engine => {
                let v: String = p.value()?;
                o.engine = Some(match v.to_ascii_lowercase().as_str() {
                    "browser" | "chrome" => ScreenEngine::Browser,
                    "ffmpeg" => ScreenEngine::Ffmpeg,
                    _ => return Err(p.error("--engine is browser or ffmpeg")),
                });
            }
            ScreenArg::Browser => o.browser = Some(p.value()?),
            ScreenArg::Ffmpeg => o.ffmpeg = Some(p.value()?),
        }
    }
    if o.share.to.is_empty() {
        return Err(p.error("at least one --to NAME|ADDR is required"));
    }
    if targets > 1 {
        return Err(p.error("give one of --display, --window and --pick"));
    }
    if o.share.player.tunnel_ttl.is_some() && o.share.player.tunnel.is_none() {
        return Err(p.error("--tunnel-ttl is for --tunnel"));
    }
    let browser_opts = BrowserOptions {
        open: o.share.open,
        ..o.share.player.clone()
    };
    if !has_browser_target(&o.share.to) && browser_opts != BrowserOptions::default() {
        return Err(p.error(PLAYER_ONLY));
    }
    match o.engine {
        Some(ScreenEngine::Ffmpeg) if o.browser.is_some() => {
            return Err(p.error("--browser is for --engine browser"));
        }
        Some(ScreenEngine::Ffmpeg) if o.target == ScreenTarget::Pick => {
            return Err(p.error("--pick needs the browser (--engine browser)"));
        }
        Some(ScreenEngine::Browser) if o.ffmpeg.is_some() => {
            return Err(p.error("--ffmpeg is for --engine ffmpeg"));
        }
        _ => {}
    }
    Ok(Command::Screen(o))
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
    fn share_options() {
        let (_, cmd) = parse_from(&[
            "share",
            "--to",
            "Office TV",
            "--to",
            "cafe TV",
            "--fps",
            "60",
            "--size",
            "1280x720",
            "--open",
            "--no-audio",
        ])
        .unwrap();
        let Command::Share(o) = cmd else { panic!() };
        assert_eq!(o.to, ["Office TV", "cafe TV"]);
        assert_eq!(o.fps, Some(Ratio::integer(60)));
        // Without URL, --open opens the share page.
        assert_eq!((o.open, o.no_audio, o.page), (true, true, None));
        assert_eq!(o.player, BrowserOptions::default());
        assert_eq!(o.size.map(|s| (s.width, s.height)), Some((1280, 720)));
        assert!(parse_from(&["share"]).is_err());
        assert!(parse_from(&["share", "https://x"]).is_err());
        // --tunnel exposes the share page; not with URL.
        let (_, cmd) = parse_from(&["share", "--to", "TV", "--tunnel"]).unwrap();
        let Command::Share(o) = cmd else { panic!() };
        assert_eq!(o.tunnel, Some(Provider::Cloudflare));
        assert!(o.player.tunnel.is_none() && !o.yes);
        let (_, cmd) = parse_from(&["share", "--to", "TV", "--tunnel=opentunnel"]).unwrap();
        let Command::Share(o) = cmd else { panic!() };
        assert_eq!(o.tunnel, Some(Provider::OpenTunnel));
        assert!(parse_from(&["share", "--to", "TV", "--tunnel=ngrok"]).is_err());
        assert_eq!(o.tunnel_ttl, Some(SHARE_TUNNEL_TTL));
        assert!(parse_from(&["share", "https://x", "--to", "TV", "--tunnel"]).is_err());
        let share = |args: &[&str]| match parse_from(args).unwrap().1 {
            Command::Share(o) => o,
            _ => panic!(),
        };
        let o = share(&["share", "--to", "TV", "--tunnel", "--tunnel-ttl", "5", "-y"]);
        assert_eq!(
            (o.tunnel_ttl, o.yes),
            (Some(Duration::from_secs(300)), true)
        );
        let o = share(&["share", "--to", "TV", "--tunnel", "--tunnel-ttl", "0"]);
        assert_eq!(o.tunnel_ttl, None);
        assert!(parse_from(&["share", "--to", "TV", "--yes"]).is_err());
        assert!(parse_from(&["share", "--to", "TV", "--tunnel-ttl", "5"]).is_err());
        assert!(parse_from(&["share", "--to", "TV", "--tunnel", "--tunnel-ttl", "-1"]).is_err());
    }

    #[test]
    fn screen_options() {
        let screen = |args: &[&str]| {
            let mut a = vec!["screen", "--to", "TV"];
            a.extend_from_slice(args);
            match parse_from(&a).map(|(_, c)| c) {
                Ok(Command::Screen(o)) => Ok(o),
                Ok(_) => panic!("not screen"),
                Err(e) => Err(e.to_string()),
            }
        };
        let o = screen(&[]).unwrap();
        assert_eq!(o.target, ScreenTarget::Display(1));
        assert_eq!(o.engine, None);
        assert_eq!(o.share.to, ["TV"]);
        assert_eq!(o.share.reconnect, DEFAULT_RECONNECT);
        assert!(o.share.page.is_none());
        assert_eq!(
            screen(&["--display", "2"]).unwrap().target,
            ScreenTarget::Display(2)
        );
        assert_eq!(
            screen(&["--display", "Bildschirm 2"]).unwrap().target,
            ScreenTarget::DisplayNamed("Bildschirm 2".into())
        );
        assert!(screen(&["--display", "0"]).unwrap_err().contains("from 1"));
        assert_eq!(
            screen(&["--window", "Keynote"]).unwrap().target,
            ScreenTarget::Window("Keynote".into())
        );
        assert!(screen(&["--window", " "]).is_err());
        assert_eq!(screen(&["--pick"]).unwrap().target, ScreenTarget::Pick);
        assert!(screen(&["--pick", "--window", "x"])
            .unwrap_err()
            .contains("one of"));
        let o = screen(&[
            "--engine",
            "ffmpeg",
            "--ffmpeg",
            "/opt/ffmpeg",
            "--fps",
            "60",
            "--size",
            "1280x720",
            "--no-audio",
            "--latency",
            "200",
            "--bitrate",
            "4000",
        ])
        .unwrap();
        assert_eq!(o.engine, Some(ScreenEngine::Ffmpeg));
        assert_eq!(o.ffmpeg, Some(PathBuf::from("/opt/ffmpeg")));
        assert!(o.share.no_audio);
        assert_eq!(o.share.fps, Some(Ratio::integer(60)));
        assert_eq!(o.share.latency_ms, Some(200));
        assert_eq!(o.share.bitrate_kbps, Some(4000));
        assert_eq!(
            screen(&["--engine", "browser", "--browser", "/x/chrome"])
                .unwrap()
                .browser,
            Some(PathBuf::from("/x/chrome"))
        );
        assert!(screen(&["--engine", "vlc"]).is_err());
        assert!(screen(&["--engine", "ffmpeg", "--pick"]).is_err());
        assert!(screen(&["--engine", "ffmpeg", "--browser", "x"]).is_err());
        assert!(screen(&["--engine", "browser", "--ffmpeg", "x"]).is_err());
        // The player's options only with --to browser.
        assert!(screen(&["--open"]).unwrap_err().contains("--to browser"));
        let o = parse_from(&["screen", "--to", "browser", "--open", "--port", "0"]).unwrap();
        let Command::Screen(o) = o.1 else { panic!() };
        assert!(o.share.open);
        assert_eq!(o.share.player.port, Some(0));
        assert!(parse_from(&["screen"]).is_err());
    }

    #[test]
    fn share_url_options() {
        let (_, cmd) = parse_from(&[
            "share",
            "https://example.com/dash",
            "--to",
            "TV",
            "--to",
            "browser",
            "--size",
            "1280x720",
            "--reload",
            "0.5",
            "--visible",
            "--profile",
            "/tmp/p",
            "--browser",
            "/opt/chrome",
            "--open",
            "--port",
            "8123",
            "--wait-for-viewer",
            "--new-token",
        ])
        .unwrap();
        let Command::Share(o) = cmd else { panic!() };
        assert_eq!(o.to, ["TV", "browser"]);
        assert_eq!(o.size.map(|s| (s.width, s.height)), Some((1280, 720)));
        assert!(o.open && !o.no_audio);
        assert_eq!(
            o.player,
            BrowserOptions {
                port: Some(8123),
                open: false,
                wait_for_viewer: true,
                new_token: true,
                tunnel: None,
                tunnel_ttl: None,
            }
        );
        assert_eq!(
            o.page,
            Some(PageOptions {
                url: "https://example.com/dash".into(),
                profile: Some("/tmp/p".into()),
                visible: true,
                reload: Some(Duration::from_secs(30)),
                browser: Some("/opt/chrome".into()),
            })
        );
        let (_, cmd) = parse_from(&["share", "--to", "TV", "file:///tmp/x.html"]).unwrap();
        let Command::Share(o) = cmd else { panic!() };
        let page = o.page.unwrap();
        assert_eq!(page.url, "file:///tmp/x.html");
        assert_eq!(
            (page.reload, page.visible, page.profile),
            (None, false, None)
        );
        let err = |args: &[&str]| parse_from(args).unwrap_err().to_string();
        assert!(err(&["share", "https://x", "--to", "TV", "--reload", "0"]).contains("--reload"));
        assert!(parse_from(&["share", "https://x", "--to", "TV", "--reload", "-1"]).is_err());
        // URL-only options without a URL.
        for opt in [
            &["--visible"][..],
            &["--profile", "/tmp/p"],
            &["--reload", "5"],
            &["--browser", "/opt/chrome"],
        ] {
            let mut args = vec!["share", "--to", "TV"];
            args.extend_from_slice(opt);
            assert!(
                err(&args).contains("'share URL'"),
                "{opt:?}: {}",
                err(&args)
            );
        }
        // With a URL there is no share page: --open is the player's.
        assert!(err(&["share", "https://x", "--to", "TV", "--open"]).contains("--open"));
        assert!(parse_from(&["share", "https://x", "--to", "browser", "--open"]).is_ok());
    }

    /// The player options mean the same in every subcommand and need
    /// `--to browser` everywhere.
    #[test]
    fn player_options_everywhere() {
        let player = |args: &[&str]| -> BrowserOptions {
            match parse_from(args).unwrap().1 {
                Command::Play(o) | Command::TestPattern(o) => o.browser,
                Command::Share(o) => BrowserOptions {
                    open: o.open,
                    ..o.player
                },
                other => panic!("{other:?}"),
            }
        };
        let want = BrowserOptions {
            port: Some(9000),
            open: true,
            wait_for_viewer: true,
            new_token: true,
            tunnel: None,
            tunnel_ttl: None,
        };
        let opts = [
            "--port",
            "9000",
            "--open",
            "--wait-for-viewer",
            "--new-token",
        ];
        for cmd in [
            &["play", "--to", "browser", "x.mp4"][..],
            &["test-pattern", "--to", "browser"],
            &["preview"],
            &["share", "--to", "browser"],
            &["share", "https://example.com", "--to", "browser"],
        ] {
            let mut args = cmd.to_vec();
            args.extend_from_slice(&opts);
            assert_eq!(player(&args), want, "{cmd:?}");
        }
        for cmd in [
            &["play", "--to", "tv", "x.mp4"][..],
            &["test-pattern", "--to", "tv"],
            &["share", "--to", "tv"],
            &["share", "https://example.com", "--to", "tv"],
        ] {
            for opt in [
                &["--port", "9000"][..],
                &["--wait-for-viewer"],
                &["--new-token"],
            ] {
                let mut args = cmd.to_vec();
                args.extend_from_slice(opt);
                let e = parse_from(&args).unwrap_err().to_string();
                assert!(e.contains("--to browser"), "{args:?}: {e}");
            }
        }
        // `browser` is no subcommand any more.
        assert!(parse_from(&["browser", "https://x", "--to", "tv"]).is_err());
    }

    #[test]
    fn reconnect_option() {
        let get = |args: &[&str]| match parse_from(args).unwrap().1 {
            Command::Play(o) | Command::TestPattern(o) => o.reconnect,
            Command::Share(o) => o.reconnect,
            other => panic!("{other:?}"),
        };
        assert_eq!(get(&["play", "--to", "tv", "x.mp4"]), DEFAULT_RECONNECT);
        assert_eq!(get(&["share", "--to", "tv"]), DEFAULT_RECONNECT);
        let secs = Duration::from_secs;
        assert_eq!(
            get(&["test-pattern", "--to", "tv", "--reconnect", "0"]),
            secs(0)
        );
        assert_eq!(get(&["share", "--to", "tv", "--reconnect", "30"]), secs(30));
        assert_eq!(
            get(&["play", "--to", "tv", "--reconnect", "1.5", "x.mp4"]),
            Duration::from_millis(1500)
        );
        for bad in ["-1", "x", "inf", "100000"] {
            assert!(parse_from(&["play", "--to", "tv", "--reconnect", bad, "x.mp4"]).is_err());
        }
        // `preview` streams to the browser only.
        assert!(parse_from(&["preview", "--reconnect", "5"]).is_err());
    }

    #[test]
    fn play_requires_to() {
        assert!(parse_from(&["play", "movie.mkv"]).is_err());
        assert!(parse_from(&["play", "--to", "tv", "-"]).is_err());
    }

    #[test]
    fn browser_receiver_and_preview() {
        let play = |args: &[&str]| match parse_from(args) {
            Ok((_, Command::Play(o) | Command::TestPattern(o))) => o,
            other => panic!("{other:?}"),
        };
        // `preview` without INPUT: the test pattern, in the browser only.
        let o = play(&["preview"]);
        assert_eq!(o.to, ["browser"]);
        assert_eq!(o.input, InputSpec::TestPattern);
        assert_eq!(o.browser, BrowserOptions::default());
        let o = play(&[
            "preview",
            "--open",
            "--port",
            "8123",
            "--latency",
            "500",
            "a.mp4",
        ]);
        assert_eq!(o.input, InputSpec::Path("a.mp4".into()));
        assert_eq!(o.latency_ms, Some(500));
        assert_eq!(
            o.browser,
            BrowserOptions {
                port: Some(8123),
                open: true,
                ..BrowserOptions::default()
            }
        );
        let o = play(&["preview", "--", "-f", "avfoundation", "-i", "1"]);
        assert!(matches!(o.input, InputSpec::FfmpegArgs(_)));
        assert!(parse_from(&["preview", "--to", "tv"]).is_err());
        // Next to TVs.
        let o = play(&[
            "play",
            "--to",
            "tv",
            "--to",
            "Browser",
            "--wait-for-viewer",
            "x.mkv",
        ]);
        assert_eq!(o.to, ["tv", "Browser"]);
        assert!(o.browser.wait_for_viewer);
        let o = play(&["test-pattern", "--to", "browser"]);
        assert_eq!(o.input, InputSpec::TestPattern);
        // The page's options need a page.
        assert!(parse_from(&["play", "--to", "tv", "--open", "x.mkv"]).is_err());
        assert!(parse_from(&["test-pattern", "--to", "tv", "--port", "1"]).is_err());
        assert!(play(&["preview", "--new-token"]).browser.new_token);
        assert_eq!(
            play(&["preview", "--tunnel"]).browser.tunnel,
            Some(Provider::Cloudflare)
        );
        assert_eq!(
            play(&["preview", "--tunnel=cloudflare"]).browser.tunnel,
            Some(Provider::Cloudflare)
        );
        assert_eq!(
            play(&["preview", "--tunnel=OpenTunnel"]).browser.tunnel,
            Some(Provider::OpenTunnel)
        );
        // The value is attached: a separate word is the input.
        assert_eq!(
            play(&["preview", "--tunnel", "x.mkv"]).input,
            InputSpec::Path("x.mkv".into())
        );
        assert!(parse_from(&["preview", "--tunnel", "opentunnel"]).is_err());
        assert!(parse_from(&["share", "--to", "TV", "--tunnel", "opentunnel"]).is_err());
        assert_eq!(play(&["preview", "--tunnel"]).browser.tunnel_ttl, None);
        assert_eq!(
            play(&["preview", "--tunnel", "--tunnel-ttl", "1.5"])
                .browser
                .tunnel_ttl,
            Some(Duration::from_secs(90))
        );
        assert!(parse_from(&["preview", "--tunnel-ttl", "5"]).is_err());
        assert!(parse_from(&["test-pattern", "--to", "tv", "--tunnel"]).is_err());
    }

    #[test]
    fn machine_flag() {
        let (g, cmd) = parse_from(&["--machine", "-v", "preview"]).unwrap();
        assert!(g.machine && g.verbose);
        assert_eq!(cmd.name(), "play");
        let (g, _) = parse_from(&["discover"]).unwrap();
        assert!(!g.machine);
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
