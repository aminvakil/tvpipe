//! `share` end to end (decision 0012, option C): a fake page (the Rust
//! one from `browser::share::tests`, built-in H.264 + PCM over a real
//! loopback WebSocket) to a fake Cast receiver and a fake AirPlay mirror
//! receiver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;

use super::tests::{fake_airplay, never, SharedTally, Tallied};
use super::*;
use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
use crate::airplay::session::MirrorOptions;
use crate::browser::share::tests::{spawn_page, PageScript, PageSeen};
use crate::cast::fake_receiver::{FakeReceiver, Options as CastFake};

fn share_opts(to: Vec<String>) -> ShareOptions {
    ShareOptions {
        to,
        latency_ms: Some(100),
        size: None,
        fps: None,
        bitrate_kbps: None,
        no_audio: false,
        port: 0,
        open: false,
    }
}

type Page = Mutex<Option<JoinHandle<PageSeen>>>;

fn page_hook(page: &Page, script: PageScript) -> impl Fn(&ShareServer) + '_ {
    move |server: &ShareServer| {
        *page.lock().unwrap() = Some(spawn_page(server.ws_url(), script.clone()));
    }
}

fn joined(page: &Page) -> PageSeen {
    page.lock().unwrap().take().unwrap().join().unwrap()
}

#[test]
fn page_to_cast_and_airplay() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let fake = FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    });
    let (cast_tally, airplay_tally) = (SharedTally::default(), SharedTally::default());
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        Ok(match t {
            Target::Cast { addr, name } => {
                Tallied::boxed(Box::new(cast::connect(*addr, name, c)?), None, &cast_tally)
            }
            Target::AirPlay { .. } => {
                let m = MirrorOptions {
                    ptp: false,
                    ..m.clone()
                };
                Tallied::boxed(
                    fake_airplay(&fake, "fake-airplay", &m)?,
                    None,
                    &airplay_tally,
                )
            }
        })
    };
    let page = Page::default();
    let script = PageScript {
        frames: 45,
        // A 4:3 capture of a large screen: scaled to fit 1920x1080.
        capture: (2880, 2160),
        ..PageScript::default()
    };
    let o = ShareOptions {
        // Small, so the built-in encoder in the fake page is quick.
        size: Some(Size {
            width: 320,
            height: 240,
        }),
        ..share_opts(vec![rx.addr.to_string(), format!("airplay:{}", fake.addr)])
    };
    share_with(o, &connect, never, &page_hook(&page, script)).unwrap();
    let seen = joined(&page);

    // The page was told what to capture, then the plan.
    let config = seen.config.unwrap();
    assert_eq!(config.get("sampleRate").unwrap().as_u32(), Some(44_100));
    let names = config.get("receivers").unwrap().as_array().unwrap();
    assert_eq!(names.len(), 2);
    let plan = seen.plan.unwrap();
    assert_eq!(plan.get("width").unwrap().as_u32(), Some(320));
    assert_eq!(plan.get("height").unwrap().as_u32(), Some(240));
    assert_eq!(plan.get("fps").unwrap().as_f64(), Some(30.0));
    // AirPlay in the session: an IDR every half second.
    assert_eq!(plan.get("keyInterval").unwrap().as_u32(), Some(15));
    let audio = plan.get("audio").unwrap();
    assert_eq!(audio.get("sampleRate").unwrap().as_u32(), Some(44_100));
    assert!(
        seen.statuses.contains(&"streaming".to_owned()),
        "{:?}",
        seen.statuses
    );
    assert_eq!(seen.frames_sent, 45);

    // Cast: every frame (nothing was lost on loopback), AAC audio.
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "STOP not received");
    let offer = st.offer.as_ref().unwrap().to_string();
    assert!(offer.contains("\"aac\""), "{offer}");
    let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
    let a = st.streams.iter().find(|s| s.codec == "aac").unwrap();
    let (tc, ta) = (*cast_tally.lock().unwrap(), *airplay_tally.lock().unwrap());
    assert_eq!(tc.video, 45, "{tc:?}");
    super::tests::assert_cast_frames(&v.frames, tc.video_accepted);
    assert!(!a.frames.is_empty());
    // AirPlay: video and PCM audio.
    assert_eq!(ta.video, 45, "{ta:?}");
    let r = fake.rec();
    assert!(r.teardown, "TEARDOWN not received");
    assert!(!r.audio.is_empty(), "AirPlay audio");
    assert_eq!(
        r.video.iter().filter(|p| p.kind == 0).count() as u64,
        ta.video_accepted
    );
}

static STOP: AtomicBool = AtomicBool::new(false);

fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Ctrl-C while streaming: the receiver gets its STOP and the page is
/// told that tvpipe stopped.
#[test]
fn stop_tells_the_page() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let tally = SharedTally::default();
    let connect = |t: &Target, c: &CastOptions, _: &MirrorOptions| -> Result<Box<dyn Sink>> {
        let Target::Cast { addr, name } = t else {
            panic!("cast only")
        };
        Ok(Tallied::boxed(
            Box::new(cast::connect(*addr, name, c)?),
            None,
            &tally,
        ))
    };
    let page = Page::default();
    let script = PageScript {
        frames: 300,
        audio: false,
        capture: (320, 180),
        ..PageScript::default()
    };
    let stopper = std::thread::spawn({
        let tally = tally.clone();
        move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while tally.lock().unwrap().video < 10 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            STOP.store(true, Ordering::SeqCst);
        }
    });
    let o = ShareOptions {
        no_audio: true,
        ..share_opts(vec![rx.addr.to_string()])
    };
    share_with(o, &connect, stop_requested, &page_hook(&page, script)).unwrap();
    stopper.join().unwrap();
    let seen = joined(&page);
    assert_eq!(seen.end.as_deref(), Some("tvpipe stopped"));
    assert!(seen.frames_sent < 300);
    assert!(seen.plan.unwrap().get("audio").unwrap().is_null());
    let state = rx.shutdown();
    assert!(state.lock().unwrap().stopped, "STOP not received");
}

/// No receiver can be reached: the page hears why.
#[test]
fn unreachable_receiver_ends_the_page() {
    let page = Page::default();
    let o = share_opts(vec!["cast:127.0.0.1:1".into()]);
    let script = PageScript {
        frames: 5,
        ..PageScript::default()
    };
    assert!(share_with(o, &connect_one, never, &page_hook(&page, script)).is_err());
    let seen = joined(&page);
    assert!(seen.plan.is_none());
    let end = seen.end.unwrap();
    assert!(end.contains("connect") || end.contains("refused"), "{end}");
}
