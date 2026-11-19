//! `share` end to end: a fake page (the Rust
//! one from `browser::share::tests`, built-in H.264 + PCM over a real
//! loopback WebSocket) to a fake Cast receiver and a fake AirPlay mirror
//! receiver.

use std::sync::Mutex;
use std::thread::JoinHandle;

use super::tests::{connect_test, fake_airplay, never, SharedTally, Tallied};
use super::*;
use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
use crate::airplay::MirrorOptions;
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
        open: false,
        ..ShareOptions::default()
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
            Target::Browser(_) => connect_test(t, c, m)?,
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
    share_with(o, &connect, &never(), &page_hook(&page, script)).unwrap();
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

static STOP: std::sync::LazyLock<crate::util::stop::Stop> =
    std::sync::LazyLock::new(crate::util::stop::Stop::new);

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
            STOP.request();
        }
    });
    let o = ShareOptions {
        no_audio: true,
        ..share_opts(vec![rx.addr.to_string()])
    };
    share_with(
        o,
        &connect,
        &Ctx::from(STOP.clone()),
        &page_hook(&page, script),
    )
    .unwrap();
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
    assert!(share_with(o, &connect_test, &never(), &page_hook(&page, script)).is_err());
    let seen = joined(&page);
    assert!(seen.plan.is_none());
    let end = seen.end.unwrap();
    assert!(end.contains("connect") || end.contains("refused"), "{end}");
}

/// Without --size, Cast is offered 1920x1080@60 (an upper bound, not the
/// capture size). The stream has the screen's shape (16:9: the fake's
/// ANSWER has no display info) and holds the capture centered at its own
/// aspect, scaled down only to fit what the receiver allows (the fake's
/// ANSWER constrains to 1280x720@30): never upscaled.
#[test]
fn stream_size_follows_capture_and_receiver_limits() {
    // (capture, stream, picture inside it at x)
    for (capture, want, picture) in [
        ((2880, 2160), (1280, 720), (160, 960, 720)),
        ((800, 600), (1066, 600), (132, 800, 600)),
        ((1920, 1200), (1280, 720), (64, 1152, 720)),
    ] {
        let rx = FakeReceiver::spawn(CastFake::default());
        let page = Page::default();
        let script = PageScript {
            frames: 3,
            audio: false,
            capture,
            ..PageScript::default()
        };
        let o = ShareOptions {
            no_audio: true,
            ..share_opts(vec![rx.addr.to_string()])
        };
        share_with(o, &connect_test, &never(), &page_hook(&page, script)).unwrap();
        let seen = joined(&page);
        let plan = seen.plan.unwrap();
        let size = (
            plan.get("width").unwrap().as_u32().unwrap(),
            plan.get("height").unwrap().as_u32().unwrap(),
        );
        assert_eq!(size, want, "capture {capture:?}");
        let c = plan.get("content").unwrap();
        let rect = |k: &str| c.get(k).unwrap().as_u32().unwrap();
        assert_eq!(
            (rect("x"), rect("y"), rect("width"), rect("height")),
            (picture.0, 0, picture.1, picture.2),
            "capture {capture:?}"
        );
        assert_eq!(plan.get("fps").unwrap().as_f64(), Some(30.0));
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        let offer = st.offer.as_ref().unwrap();
        let video = offer["offer"]["supportedStreams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["type"].as_str() == Some("video_source"))
            .unwrap()
            .clone();
        let res = &video["resolutions"].as_array().unwrap()[0];
        assert_eq!(
            (res["width"].as_u32(), res["height"].as_u32()),
            (Some(1920), Some(1080)),
            "{video:?}"
        );
        assert_eq!(video["maxFrameRate"].as_str(), Some("60"), "{video:?}");
    }
}

#[test]
fn share_offer_bounds() {
    let o = share_opts(vec!["tv".into()]);
    let offer = ShareOffer::new(&o);
    assert_eq!(
        (offer.size, offer.fps),
        (
            Size {
                width: 1920,
                height: 1080
            },
            Ratio::integer(60)
        )
    );
    let o = ShareOptions {
        size: Some(Size {
            width: 1280,
            height: 720,
        }),
        fps: Some(Ratio::integer(25)),
        bitrate_kbps: Some(3000),
        ..o
    };
    let offer = ShareOffer::new(&o);
    assert_eq!(
        (offer.size.width, offer.fps, offer.bitrate_kbps),
        (1280, Ratio::integer(25), 3000)
    );
}

/// Browser players take any shape: alone, the stream keeps the capture's
/// (a Safari window of 1186x864 was pillarboxed to 1536x864 before);
/// next to a TV, the TV's shape wins.
#[test]
fn browser_players_follow_the_capture() {
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        match t {
            Target::Browser(o) => {
                let o = PlayerOptions {
                    port: Some(0),
                    ..(**o).clone()
                };
                Ok(Box::new(BrowserSink::bind(&o, &never())?))
            }
            _ => connect_test(t, c, m),
        }
    };
    let script = PageScript {
        frames: 3,
        audio: false,
        capture: (1186, 864),
        ..PageScript::default()
    };
    let content = |plan: &crate::codec::json::Value| {
        let c = plan.get("content").unwrap();
        let v = |k: &str| c.get(k).unwrap().as_u32().unwrap();
        (v("x"), v("y"), v("width"), v("height"))
    };
    let size = |plan: &crate::codec::json::Value| {
        let v = |k: &str| plan.get(k).unwrap().as_u32().unwrap();
        (v("width"), v("height"))
    };
    let page = Page::default();
    let o = ShareOptions {
        no_audio: true,
        ..share_opts(vec!["browser".into()])
    };
    share_with(o, &connect, &never(), &page_hook(&page, script.clone())).unwrap();
    let plan = joined(&page).plan.unwrap();
    assert_eq!(size(&plan), (1186, 864));
    assert_eq!(content(&plan), (0, 0, 1186, 864));

    // With a TV (16:9, at most 1280x720 by the fake's ANSWER).
    let rx = FakeReceiver::spawn(CastFake::default());
    let page = Page::default();
    let o = ShareOptions {
        no_audio: true,
        ..share_opts(vec!["browser".into(), rx.addr.to_string()])
    };
    share_with(o, &connect, &never(), &page_hook(&page, script)).unwrap();
    let plan = joined(&page).plan.unwrap();
    assert_eq!(size(&plan), (1280, 720));
    assert!(content(&plan).0 > 0, "{plan}");
    rx.shutdown();
}

/// A capture over the tunnel waits for the local yes: the first one is
/// turned down (its page is told), the next one is shown, with at least
/// the remote latency floor.
#[test]
fn remote_shares_wait_for_the_local_yes() {
    let latencies = Mutex::new(Vec::new());
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        match t {
            Target::Browser(o) => {
                latencies.lock().unwrap().push(o.latency_ms);
                let o = PlayerOptions {
                    port: Some(0),
                    ..(**o).clone()
                };
                Ok(Box::new(BrowserSink::bind(&o, &never())?))
            }
            _ => connect_test(t, c, m),
        }
    };
    type Pages = std::sync::Arc<Mutex<Option<JoinHandle<(PageSeen, PageSeen)>>>>;
    let pages: Pages = Default::default();
    let p = pages.clone();
    let hook = move |server: &ShareServer| -> Result<Box<dyn std::any::Any>> {
        let public = server.publish("remote.example", None, None).unwrap();
        let token = public.rsplit("t=").next().unwrap().to_owned();
        let local = server.ws_url();
        let ws = format!("{}?t={token}", local.split('?').next().unwrap());
        let script = PageScript {
            frames: 3,
            audio: false,
            public_host: Some("remote.example".into()),
            ..PageScript::default()
        };
        *p.lock().unwrap() = Some(std::thread::spawn(move || {
            let first = spawn_page(ws.clone(), script.clone()).join().unwrap();
            let second = spawn_page(ws, script).join().unwrap();
            (first, second)
        }));
        Ok(Box::new(()))
    };
    let asked = Mutex::new(Vec::<CaptureInfo>::new());
    let approve = |cap: &CaptureInfo, _gone: &dyn Fn() -> bool| {
        let mut a = asked.lock().unwrap();
        a.push(cap.clone());
        a.len() > 1
    };
    let o = ShareOptions {
        no_audio: true,
        ..share_opts(vec!["browser".into()])
    };
    share_rejoin(
        o,
        &connect,
        &never(),
        &hook,
        None,
        ShareKind::Page,
        &approve,
    )
    .unwrap();
    let (first, second) = pages.lock().unwrap().take().unwrap().join().unwrap();
    assert!(first.plan.is_none(), "{first:?}");
    assert_eq!(
        first.end.as_deref(),
        Some("the host did not accept the share")
    );
    assert!(second.plan.is_some(), "{second:?}");
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    let remote = asked[0].remote.as_ref().unwrap();
    assert_eq!(remote.peer, None);
    assert!(remote.agent.contains("Chrome/131"), "{remote:?}");
    // --latency 100 was asked for; a remote share gets the floor.
    assert_eq!(*latencies.lock().unwrap(), vec![REMOTE_SHARE_LATENCY_MS]);
}
