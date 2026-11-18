//! `browser` end to end with the browser replaced by fake pages (the Rust
//! one from `browser::share::tests`): the receivers connect first, the
//! page is sized from them, a page that goes away (a navigation) is
//! replaced by the next one on the same timeline, and the session ends
//! when the browser is gone.

use std::sync::Mutex;
use std::thread::JoinHandle;

use super::tests::{fake_airplay, never, SharedTally, Tallied};
use super::*;
use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
use crate::airplay::session::MirrorOptions;
use crate::browser::share::tests::{spawn_page, PageScript, PageSeen};
use crate::cast::fake_receiver::{FakeReceiver, Options as CastFake};
use crate::cli::BrowseOptions;

fn browse_opts(to: Vec<String>) -> BrowseOptions {
    BrowseOptions {
        url: "http://127.0.0.1:1/".into(),
        to,
        latency_ms: Some(100),
        size: None,
        fps: None,
        bitrate_kbps: None,
        no_audio: false,
        profile: None,
        visible: false,
        reload: None,
        browser: None,
    }
}

/// Fake pages one after the other (each a document of the tab), then the
/// browser "closes" (`ShareHandle::end`). Joined when dropped.
struct FakeBrowser(
    Option<JoinHandle<Vec<PageSeen>>>,
    std::sync::Arc<Mutex<Vec<PageSeen>>>,
);

impl Drop for FakeBrowser {
    fn drop(&mut self) {
        if let Some(t) = self.0.take() {
            let seen = t.join().unwrap();
            *self.1.lock().unwrap() = seen;
        }
    }
}

#[test]
fn pages_to_cast_and_airplay_across_a_navigation() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let fake = FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    });
    let (cast_tally, airplay_tally) = (SharedTally::default(), SharedTally::default());
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        Ok(match t {
            Target::Browser(_) => connect_one(t, c, m)?,
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
    let viewport = Mutex::new(None);
    let seen: std::sync::Arc<Mutex<Vec<PageSeen>>> = Default::default();
    let launcher = |server: &ShareServer, vp: (u32, u32)| -> Result<Box<dyn std::any::Any>> {
        *viewport.lock().unwrap() = Some(vp);
        let url = server.ws_url();
        let handle = server.handle();
        let t = std::thread::spawn(move || {
            let script = PageScript {
                frames: 20,
                capture: vp,
                send_ended: false,
                ..PageScript::default()
            };
            let first = spawn_page(url.clone(), script.clone()).join().unwrap();
            let second = spawn_page(url, script).join().unwrap();
            handle.end("the browser was closed");
            vec![first, second]
        });
        Ok(Box::new(FakeBrowser(Some(t), seen.clone())))
    };
    let o = BrowseOptions {
        // Small, so the built-in encoder in the fake page is quick.
        size: Some(Size {
            width: 320,
            height: 180,
        }),
        ..browse_opts(vec![rx.addr.to_string(), format!("airplay:{}", fake.addr)])
    };
    browse::browse_with(&o, &connect, never, &launcher).unwrap();
    assert_eq!(*viewport.lock().unwrap(), Some((320, 180)));
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(seen.len(), 2);
    for (i, s) in seen.iter().enumerate() {
        assert!(!s.busy, "page {i} was told busy");
        let plan = s
            .plan
            .as_ref()
            .unwrap_or_else(|| panic!("page {i}: no plan"));
        assert_eq!(plan.get("width").unwrap().as_u32(), Some(320));
        assert_eq!(plan.get("height").unwrap().as_u32(), Some(180));
        // The page fills the frame: no bars.
        let c = plan.get("content").unwrap();
        assert_eq!(c.get("width").unwrap().as_u32(), Some(320));
        assert_eq!(c.get("x").unwrap().as_u32(), Some(0));
        assert_eq!(s.frames_sent, 20);
    }
    let config = seen[0].config.as_ref().unwrap();
    assert_eq!(config.get("sampleRate").unwrap().as_u32(), Some(44_100));
    assert_eq!(config.get("fps").unwrap().as_f64(), Some(30.0));

    // Both pages' frames reached both receivers.
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "STOP not received");
    assert!(st.streams.iter().any(|s| s.codec == "aac"), "Cast audio");
    let (tc, ta) = (*cast_tally.lock().unwrap(), *airplay_tally.lock().unwrap());
    assert_eq!(tc.video, 40, "{tc:?}");
    assert_eq!(ta.video, 40, "{ta:?}");
    let r = fake.rec();
    assert!(r.teardown, "TEARDOWN not received");
    assert!(!r.audio.is_empty(), "AirPlay audio");
}

#[test]
fn page_size_follows_the_receivers_and_a_browser_failure_stops_them() {
    // The fake receiver's ANSWER allows 1280x720: the page is that.
    let rx = FakeReceiver::spawn(CastFake::default());
    let viewport = Mutex::new(None);
    let launcher = |_: &ShareServer, vp: (u32, u32)| -> Result<Box<dyn std::any::Any>> {
        *viewport.lock().unwrap() = Some(vp);
        Err(Error::not_found(
            "browser: cannot open x: net::ERR_NAME_NOT_RESOLVED",
        ))
    };
    let o = BrowseOptions {
        no_audio: true,
        ..browse_opts(vec![rx.addr.to_string()])
    };
    let e = browse::browse_with(&o, &connect_one, never, &launcher).unwrap_err();
    assert!(e.to_string().contains("ERR_NAME_NOT_RESOLVED"), "{e}");
    assert_eq!(*viewport.lock().unwrap(), Some((1280, 720)));
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "the receiver was not stopped");
    // The offer was the most the browser mode sends: 1920x1080@30.
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
    assert_eq!(video["maxFrameRate"].as_str(), Some("30"), "{video:?}");
}

#[test]
fn a_browser_that_fails_later_fails_the_session() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let launcher = |server: &ShareServer, _: (u32, u32)| -> Result<Box<dyn std::any::Any>> {
        let handle = server.handle();
        // The page never shares; the keeper gives up.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            handle.fail(Error::protocol(
                "the page cannot share itself: NotAllowedError",
            ));
        });
        Ok(Box::new(()))
    };
    let o = BrowseOptions {
        no_audio: true,
        ..browse_opts(vec![rx.addr.to_string()])
    };
    let e = browse::browse_with(&o, &connect_one, never, &launcher).unwrap_err();
    assert!(e.to_string().contains("NotAllowedError"), "{e}");
    let state = rx.shutdown();
    assert!(state.lock().unwrap().stopped);
}
