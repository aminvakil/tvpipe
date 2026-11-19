//! `screen` end to end: a fake page (as for `share`) or, with
//! `--ignored` and `TVPIPE_TEST_SCREEN=1`, tvpipe's real browser capturing
//! this computer's screen, to a fake Cast receiver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::tests::{connect_test, never, SharedTally, Tallied};
use super::*;
use crate::airplay::MirrorOptions;
use crate::browser::share::tests::{spawn_page, PageScript, PageSeen};
use crate::cast::fake_receiver::{FakeReceiver, Options as CastFake};

fn opts(to: Vec<String>) -> ShareOptions {
    ShareOptions {
        to,
        latency_ms: Some(100),
        ..ShareOptions::default()
    }
}

/// Sets its flag when dropped (the browser guard of `screen`).
struct Guard(Arc<AtomicBool>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn cast_only<'a>(
    tally: &'a SharedTally,
) -> impl Fn(&Target, &CastOptions, &MirrorOptions) -> Result<Box<dyn Sink>> + Sync + 'a {
    move |t: &Target, c: &CastOptions, _: &MirrorOptions| {
        let Target::Cast { addr, name } = t else {
            panic!("cast only")
        };
        Ok(Tallied::boxed(
            Box::new(cast::connect(*addr, name, c)?),
            None,
            tally,
        ))
    }
}

/// The page tvpipe's browser would show streams like a share page; the
/// browser (the source's guard) is stopped at the end.
#[test]
fn screen_page_to_cast() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let tally = SharedTally::default();
    let connect = cast_only(&tally);
    let page: Mutex<Option<JoinHandle<PageSeen>>> = Mutex::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let script = PageScript {
        frames: 30,
        capture: (640, 400),
        ..PageScript::default()
    };
    let source = |server: &ShareServer| -> Result<Box<dyn std::any::Any>> {
        // The screen page is served with the token.
        assert!(server.screen_url().contains("/screen?t="));
        *page.lock().unwrap() = Some(spawn_page(server.ws_url(), script.clone()));
        Ok(Box::new(Guard(dropped.clone())))
    };
    let o = ShareOptions {
        size: Some(Size {
            width: 320,
            height: 200,
        }),
        ..opts(vec![rx.addr.to_string()])
    };
    screen::screen_with(o, &connect, &never(), &source).unwrap();
    assert!(
        dropped.load(Ordering::SeqCst),
        "the browser was not stopped"
    );
    let seen = page.lock().unwrap().take().unwrap().join().unwrap();
    assert_eq!(seen.frames_sent, 30);
    let plan = seen.plan.unwrap();
    assert_eq!(plan.get("width").unwrap().as_u32(), Some(320));
    assert!(!plan.get("audio").unwrap().is_null());
    let t = *tally.lock().unwrap();
    assert_eq!(t.video, 30, "{t:?}");
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "STOP not received");
    assert!(st
        .streams
        .iter()
        .any(|s| s.codec == "aac" && !s.frames.is_empty()));
}

/// The browser cannot capture (no permission, the picker cancelled):
/// the session fails with its reason before any receiver is connected.
#[test]
fn a_capture_that_fails_fails_the_session() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let tally = SharedTally::default();
    let connect = cast_only(&tally);
    let source = |server: &ShareServer| -> Result<Box<dyn std::any::Any>> {
        let h = server.handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            h.fail(Error::protocol(
                "the browser cannot capture: NotAllowedError: Permission denied",
            ));
        });
        Ok(Box::new(()))
    };
    let e = screen::screen_with(opts(vec![rx.addr.to_string()]), &connect, &never(), &source)
        .unwrap_err();
    assert!(e.to_string().contains("NotAllowedError"), "{e}");
    assert_eq!(tally.lock().unwrap().video, 0);
    let state = rx.shutdown();
    assert!(
        state.lock().unwrap().offer.is_none(),
        "a receiver was connected"
    );

    // A browser that cannot start: its error.
    let source = |_: &ShareServer| -> Result<Box<dyn std::any::Any>> {
        Err(Error::not_found("no Chromium-based browser found"))
    };
    let e = screen::screen_with(
        opts(vec!["cast:127.0.0.1:1".into()]),
        &connect_test,
        &never(),
        &source,
    )
    .unwrap_err();
    assert!(e.to_string().contains("no Chromium-based browser"), "{e}");
}

/// The browser's "Stop sharing" ends the session like the end of a file.
#[test]
fn stopping_in_the_browser_ends_the_session() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let tally = SharedTally::default();
    let connect = cast_only(&tally);
    let source = |server: &ShareServer| -> Result<Box<dyn std::any::Any>> {
        let page = spawn_page(
            server.ws_url(),
            PageScript {
                frames: 10_000,
                audio: false,
                capture: (320, 180),
                ..PageScript::default()
            },
        );
        let h = server.handle();
        let tally = tally.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while tally.lock().unwrap().video < 10 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            h.end("sharing was stopped in the browser");
        });
        Ok(Box::new(page))
    };
    let o = ShareOptions {
        no_audio: true,
        ..opts(vec![rx.addr.to_string()])
    };
    let t0 = Instant::now();
    screen::screen_with(o, &connect, &never(), &source).unwrap();
    assert!(t0.elapsed() < Duration::from_secs(20));
    let state = rx.shutdown();
    assert!(state.lock().unwrap().stopped, "STOP not received");
}

static STOP: std::sync::LazyLock<crate::util::stop::Stop> =
    std::sync::LazyLock::new(crate::util::stop::Stop::new);

/// The real thing: tvpipe's browser captures this computer's main screen
/// (and its sound) for a few seconds. Needs a desktop session and the
/// screen-recording permission for the browser:
/// `TVPIPE_TEST_SCREEN=1 cargo test chrome_captures_the_screen -- --ignored`
/// (`TVPIPE_TEST_SCREEN=window:TITLE` captures that window).
#[test]
#[ignore]
fn chrome_captures_the_screen() {
    let Ok(what) = std::env::var("TVPIPE_TEST_SCREEN") else {
        eprintln!("TVPIPE_TEST_SCREEN not set; skipped");
        return;
    };
    let target = match what.strip_prefix("window:") {
        Some(t) => crate::opts::ScreenTarget::Window(t.into()),
        None => crate::opts::ScreenTarget::Display(1),
    };
    let bin = crate::browser::launch::find_browser(None).unwrap();
    let rx = FakeReceiver::spawn(CastFake::default());
    let tally = SharedTally::default();
    let connect = cast_only(&tally);
    let profile: Mutex<Option<std::path::PathBuf>> = Mutex::default();
    let source = |server: &ShareServer| -> Result<Box<dyn std::any::Any>> {
        let b = crate::browser::screen::ScreenBrowser::start(
            crate::browser::screen::ScreenOptions {
                bin: bin.clone(),
                target: target.clone(),
            },
            server,
            STOP.clone(),
        )?;
        *profile.lock().unwrap() = Some(b.profile_dir().to_owned());
        Ok(Box::new(b))
    };
    let stopper = std::thread::spawn({
        let tally = tally.clone();
        move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            while tally.lock().unwrap().video < 60 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            STOP.request();
        }
    });
    screen::screen_with(
        opts(vec![rx.addr.to_string()]),
        &connect,
        &Ctx::from(STOP.clone()),
        &source,
    )
    .unwrap();
    stopper.join().unwrap();
    let t = *tally.lock().unwrap();
    assert!(t.video >= 60, "{t:?}");
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "STOP not received");
    let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
    assert!(v.frames.len() >= 30, "{}", v.frames.len());
    // The temporary profile is gone with the browser.
    let dir = profile.lock().unwrap().clone().unwrap();
    assert!(!dir.exists(), "{} left behind", dir.display());
}
