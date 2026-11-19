//! `tvpipe share URL` (W13): the share
//! pipeline with tvpipe's own browser as the page.
//!
//! 1. Resolve and connect the receivers first: the stream's size is the
//!    receivers' screen shape at most 1920x1080 (or `--size`), within
//!    their limits (W12's sizing, `docs/share-protocol.md`).
//! 2. Start the browser with its viewport at exactly that size
//!    (`browser::headless`), so the tab fills the frame (no bars), and
//!    open URL; the page shares its tab to the [`ShareServer`] in rejoin
//!    mode (navigations, reloads and crashes share again).
//! 3. Stream like `share` (encoded by the browser; PCM for AirPlay and
//!    the player, AAC for Cast) until Ctrl-C / `q`, every receiver is
//!    gone, or the browser gives up; then close the browser and delete
//!    its temporary profile.

use std::any::Any;

use super::*;
use crate::browser::headless::{self, Headless, HeadlessOptions};
use crate::browser::launch::{self, LaunchOptions};

/// The page's size without `--size`, before the receivers' shape.
const BROWSE_SIZE: Size = Size {
    width: 1920,
    height: 1080,
};
const BROWSE_FPS: Ratio = Ratio::integer(30);

/// Starts the page source for a viewport; the guard stops it when
/// dropped (the tests start fake pages instead of a browser).
type Launcher<'a> = &'a dyn Fn(&ShareServer, (u32, u32)) -> Result<Box<dyn Any>>;

/// `share URL`.
pub(super) fn browse(opts: ShareOptions, ctx: &Ctx) -> Result<()> {
    let stop = ctx.stop.clone();
    let page = opts
        .page
        .clone()
        .ok_or_else(|| Error::usage("share: no URL"))?;
    let url = headless::page_url(&page.url)?;
    headless::check_url(&url)?;
    // Problems with the browser before any receiver is bothered.
    let bin = launch::find_browser(page.browser.as_deref())?;
    debug!("browser: using {}", bin.display());
    // Profiles (and browsers) a killed tvpipe left behind.
    launch::sweep_stale_profiles();
    let o = page;
    let launcher = move |server: &ShareServer, viewport: (u32, u32)| -> Result<Box<dyn Any>> {
        let h = Headless::start(
            HeadlessOptions {
                url: url.clone(),
                launch: LaunchOptions {
                    bin: bin.clone(),
                    viewport,
                    visible: o.visible,
                    screen: None,
                },
                profile: o.profile.clone(),
                reload: o.reload,
            },
            server.handle(),
            stop.clone(),
        )?;
        Ok(Box::new(h))
    };
    let rejoin = Rejoin::system(opts.reconnect, ctx);
    let connect = |t: &Target, o: &CastOptions, m: &crate::airplay::session::MirrorOptions| {
        connect_one(t, o, m, ctx)
    };
    browse_rejoin(&opts, &connect, ctx, &launcher, Some(&rejoin))
}

/// `share URL` with the connector, stop request and page source given.
pub(super) fn browse_with(
    opts: &ShareOptions,
    connect: Connector<'_>,
    ctx: &Ctx,
    launch: Launcher<'_>,
) -> Result<()> {
    browse_rejoin(opts, connect, ctx, launch, None)
}

/// `browse_with` with the reconnect hooks.
fn browse_rejoin(
    opts: &ShareOptions,
    connect: Connector<'_>,
    ctx: &Ctx,
    launch: Launcher<'_>,
    rejoin: Option<&Rejoin>,
) -> Result<()> {
    crate::util::timer::raise_resolution();
    let stop = &ctx.stop;
    let mut server = ShareServer::bind(0)?;
    server.set_rejoin(true);
    let fail = |server: &mut ShareServer, e: Error| {
        server.set_end_reason(&e.to_string());
        e
    };
    let mut targets =
        resolve_all(&opts.to, rejoin, &ctx.events).map_err(|e| fail(&mut server, e))?;
    let names: Vec<String> = targets.iter().map(|t| target_name(t).to_owned()).collect();
    let pcm = RawAudioSpec {
        sample_rate: builtin::pattern_sample_rate(
            targets.iter().any(|t| matches!(t, Target::AirPlay { .. })),
        ),
        channels: 2,
        sample_fmt: "s16le".into(),
    };
    let audio = !opts.no_audio;
    let fps = opts.fps.unwrap_or(BROWSE_FPS);
    server.configure(&PageConfig {
        fps: fps.as_f64(),
        audio,
        sample_rate: pcm.sample_rate,
        channels: pcm.channels,
        receivers: names,
    });
    // The offer: --size/--fps/--bitrate or 1920x1080@30; the ANSWERs
    // and AirPlay's canvas narrow it.
    let offer = ShareOffer::new(&ShareOptions {
        size: Some(opts.size.unwrap_or(BROWSE_SIZE)),
        fps: Some(fps),
        ..opts.clone()
    });
    let budget = shared_budget(opts.latency_ms, &targets);
    let d = Desired {
        size: offer.size,
        fps,
        bitrate_kbps: offer.bitrate_kbps,
        latency_ms: budget.session_ms,
    };
    let mut cast_opts = share_cast_options(&d, &offer, audio, &pcm);
    cast_opts.latency_ms = budget.cast_ms;
    configure_browsers(
        &mut targets,
        &player_options(opts),
        budget.browser_ms,
        audio,
        &ctx.storage,
    );
    let mirror = crate::airplay::session::MirrorOptions {
        latency_ms: budget.airplay_ms,
        audio,
        ..Default::default()
    };
    let sinks = connect_all(&targets, connect, &cast_opts, &mirror, rejoin, &ctx.events)
        .map_err(|e| fail(&mut server, e))?;
    if stop.requested() {
        stop_all(sinks);
        return Ok(());
    }
    let caps: Vec<&Capabilities> = sinks.iter().map(|s| s.capabilities()).collect();
    let named: Vec<(&str, &Capabilities)> =
        sinks.iter().map(|s| (s.name(), s.capabilities())).collect();
    let limits = receiver_limits(&caps);
    // Only browser players: the page keeps BROWSE_SIZE's (or --size's)
    // shape.
    let (screen, max) = screen_shape(&caps);
    let aspect = screen.unwrap_or_else(|| {
        let s = opts.size.unwrap_or(BROWSE_SIZE);
        crate::types::aspect_of(s.width, s.height).unwrap_or(layout::DEFAULT_ASPECT)
    });
    let d = Desired {
        size: opts.size.unwrap_or_else(|| {
            let (width, height) =
                layout::output_size((BROWSE_SIZE.width, BROWSE_SIZE.height), aspect, max);
            Size { width, height }
        }),
        ..d
    };
    let planned = share_plan(&d, &caps, opts.bitrate_kbps.is_some());
    let (families, warnings) = if audio {
        input::encoded_audio_families(Some(&pcm), &named)
    } else {
        (Vec::new(), Vec::new())
    };
    drop((caps, named));
    let plan = match planned {
        Ok(p) => p,
        Err(e) => {
            stop_all(sinks);
            return Err(fail(&mut server, e));
        }
    };
    for w in warnings {
        warn!("{w}");
    }
    let viewport = (plan.video.width, plan.video.height);
    info!(
        "browser: receivers allow {limits}, {}; the page is {}x{}",
        screen_label(screen),
        viewport.0,
        viewport.1
    );
    let t0 = Instant::now();
    let source = match launch(&server, viewport) {
        Ok(s) => s,
        Err(e) => {
            stop_all(sinks);
            return Err(fail(&mut server, e));
        }
    };
    let Some(cap) = server.wait_capture(stop) else {
        stop_all(sinks);
        drop(source);
        return match server.failure() {
            Some(e) => Err(e),
            None => Ok(()),
        };
    };
    capture_event(&cap, &ctx.events);
    debug!(
        "browser: the page shares {} ms after the start",
        t0.elapsed().as_millis()
    );
    let page_audio = (!families.is_empty() && cap.audio).then_some((pcm.sample_rate, pcm.channels));
    if !families.is_empty() && !cap.audio {
        warn!("browser: the tab's capture has no sound");
    }
    let content = layout::place(
        (cap.width, cap.height),
        (plan.video.width, plan.video.height),
    );
    // Usually smaller at first: Chrome's "sharing this tab" bar takes
    // room until the browser keeper resizes the window, and the page then
    // fills the frame again.
    if (content.width, content.height) != viewport {
        debug!(
            "browser: the tab is {}x{}, not {}x{}; it fills {}x{} at +{}+{}",
            cap.width,
            cap.height,
            viewport.0,
            viewport.1,
            content.width,
            content.height,
            content.x,
            content.y
        );
    }
    info!(
        "plan: {}x{}@{} {} kbit/s, keyframe every {} frames, {}, latency {} ms (encoded by the browser)",
        plan.video.width,
        plan.video.height,
        plan.video.fps,
        plan.video.bitrate_kbps,
        plan.video.gop,
        match page_audio {
            Some((rate, ch)) => format!("PCM {rate} Hz {ch} ch"),
            None => "no audio".into(),
        },
        plan.latency_ms
    );
    let share_plan = SharePlan {
        width: plan.video.width,
        height: plan.video.height,
        fps: plan.video.fps.as_f64(),
        bitrate_bps: plan.video.bitrate_kbps as u64 * 1000,
        key_interval: plan.video.gop,
        audio: page_audio,
        content,
    };
    let opened = server.start(
        &share_plan,
        ShareInputOptions {
            fps: plan.video.fps,
            audio: page_audio.map(|_| pcm.clone()),
            audio_families: if page_audio.is_some() {
                families
            } else {
                Vec::new()
            },
        },
        stop.clone(),
    );
    let input = match opened {
        Ok(i) => i,
        Err(e) => {
            stop_all(sinks);
            drop(source);
            if stop.requested() {
                info!("stopped");
                return Ok(());
            }
            return Err(e);
        }
    };
    let r = stream(
        Box::new(input),
        events::Route::Browser,
        plan.latency_ms,
        sinks,
        ctx,
    );
    drop(source);
    match r {
        Ok(EndReason::Interrupted) => {
            info!("stopped");
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}
