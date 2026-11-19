//! The party's main screen on a TV: the video screen (rendered here,
//! [`crate::screen`], streamed with [`tvpipe::Stream`]) or the web
//! screen (`web/screen/`, shown and captured by tvpipe's own browser with
//! [`tvpipe::share`]). Either runs on its own thread and starts again
//! when the session ends (every receiver gone), without touching the
//! party.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tvpipe::{Options, PixelFormat, PlayerOptions, SessionState, Stream};
use tvpipe_canvas::Canvas;
use tvpipe_party::ScreenWatch;

use crate::screen::Screen;
use crate::sound::{Cues, Mixer, CHANNELS, RATE};

/// Where and how the screen goes.
#[derive(Debug, Clone)]
pub struct TvOptions {
    /// The receivers (`tvpipe` targets: names, addresses, `browser`).
    pub to: Vec<String>,
    /// The video size.
    pub width: u32,
    /// The video size.
    pub height: u32,
    /// Frames per second while something moves.
    pub fps: u32,
    /// Open the `browser` target's page.
    pub open: bool,
    /// Report sessions and render times on stderr.
    pub verbose: bool,
}

impl Default for TvOptions {
    fn default() -> TvOptions {
        TvOptions {
            to: Vec::new(),
            width: 1280,
            height: 720,
            fps: 30,
            open: false,
            verbose: false,
        }
    }
}

impl TvOptions {
    fn options(&self) -> Options {
        let mut o = Options::new();
        for t in &self.to {
            o = o.to(t.as_str());
        }
        o.player(PlayerOptions::new().open(self.open))
    }
}

/// A screen on the TV, until dropped.
pub struct Tv {
    stop: Arc<AtomicBool>,
    url: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Tv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tv").finish_non_exhaustive()
    }
}

impl Tv {
    /// The video screen of this party's screen view.
    pub fn video(watch: ScreenWatch, o: TvOptions) -> Tv {
        Tv::spawn(o, move |o, stop, url| {
            let options = o
                .options()
                .video(o.width, o.height, o.fps, PixelFormat::Rgba)
                .audio(RATE, CHANNELS);
            let mut stream = Stream::start(options).map_err(|e| e.to_string())?;
            render(&mut stream, &watch, o, stop, url);
            if stop.load(Ordering::Relaxed) {
                let _ = stream.stop();
            }
            Ok(())
        })
    }

    /// The web screen at `url` ([`Party::screen_url`](tvpipe_party::Party::screen_url)).
    pub fn web(screen_url: String, o: TvOptions) -> Tv {
        Tv::spawn(o, move |o, stop, url| {
            let options = o.options().size(o.width, o.height);
            let session = tvpipe::share(Some(&screen_url), options).map_err(|e| e.to_string())?;
            while !stop.load(Ordering::Relaxed) {
                if let Some(u) = session.player_url() {
                    *url.lock().unwrap_or_else(|e| e.into_inner()) = Some(u);
                }
                if session.wait_timeout(Duration::from_millis(200)) {
                    return session.wait().map(|_| ()).map_err(|e| e.to_string());
                }
            }
            let _ = session.stop();
            Ok(())
        })
    }

    fn spawn(
        o: TvOptions,
        session: impl Fn(&TvOptions, &AtomicBool, &Mutex<Option<String>>) -> Result<(), String>
            + Send
            + 'static,
    ) -> Tv {
        let stop = Arc::new(AtomicBool::new(false));
        let url = Arc::new(Mutex::new(None));
        let (s, u) = (stop.clone(), url.clone());
        let thread = thread::Builder::new()
            .name("drawbluff-tv".into())
            .spawn(move || {
                let mut wait = Duration::from_secs(2);
                while !s.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    match session(&o, &s, &u) {
                        Ok(()) if o.verbose => eprintln!("drawbluff: the TV session ended"),
                        Ok(()) => {}
                        Err(e) => eprintln!("drawbluff: the TV: {e}"),
                    }
                    if started.elapsed() > Duration::from_secs(60) {
                        wait = Duration::from_secs(2);
                    }
                    // Again, slowly (a receiver turned off, tvpipe failed).
                    let until = Instant::now() + wait;
                    while Instant::now() < until && !s.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(100));
                    }
                    wait = (wait * 2).min(Duration::from_secs(30));
                }
            })
            .ok();
        Tv { stop, url, thread }
    }

    /// The `browser` target's page, once the session runs.
    pub fn player_url(&self) -> Option<String> {
        self.url.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Ends the session (every receiver gets its teardown).
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Tv {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The party's clock ([`Party::now`](tvpipe_party::Party::now)).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Draws the latest screen view when it (or its countdown) changed,
/// writes frames as the encoder takes them and the cues over silence,
/// until the session ends or `stop`.
fn render(
    stream: &mut Stream,
    watch: &ScreenWatch,
    o: &TvOptions,
    stop: &AtomicBool,
    url: &Mutex<Option<String>>,
) {
    let mut screen = Screen::new();
    let mut canvas = Canvas::new(o.width, o.height);
    let mut cues = Cues::default();
    let mut mixer = Mixer::default();
    let mut last = None;
    // Audio: from the first write on, as much as the clock asks for
    // (plus a little lead).
    let mut audio: Option<(Duration, u64)> = None;
    let (mut drawn, mut spent) = (0u32, Duration::ZERO);
    let frame = Duration::from_secs(1) / o.fps.max(1);
    while !stop.load(Ordering::Relaxed) && stream.state() != SessionState::Ended {
        if let Some(u) = stream.player_url() {
            let mut url = url.lock().unwrap_or_else(|e| e.into_inner());
            if url.as_deref() != Some(&u) {
                *url = Some(u);
            }
        }
        let now = now_ms();
        let (rev, view) = watch.get();
        let key = (rev, Screen::clock(&view, now));
        if last != Some(key) {
            let t = Instant::now();
            screen.render(&mut canvas, &view, now);
            spent += t.elapsed();
            drawn += 1;
            if o.verbose && drawn % 100 == 0 {
                eprintln!(
                    "drawbluff: {drawn} frames drawn, {:.1} ms each",
                    spent.as_secs_f64() * 1000.0 / f64::from(drawn)
                );
            }
            if let Some(cue) = cues.update(&view) {
                mixer.play(&cue);
            }
            last = Some(key);
        }
        if stream.wants_frame() {
            let pts = stream.elapsed();
            let _ = stream.write_video(canvas.data(), pts);
        }
        let elapsed = stream.elapsed();
        let (start, written) = audio.get_or_insert((elapsed, 0));
        let due = ((elapsed + Duration::from_millis(60))
            .saturating_sub(*start)
            .as_secs_f64()
            * f64::from(RATE)) as u64;
        if due > *written {
            let frames = due - *written;
            let pts = *start + Duration::from_secs_f64(*written as f64 / f64::from(RATE));
            let _ = stream.write_audio(&mixer.take(frames as usize), pts);
            *written = due;
        }
        thread::sleep(frame / 3);
    }
}
