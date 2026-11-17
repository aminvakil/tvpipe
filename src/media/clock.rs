//! The sender's media clock: maps stream timestamps to monotonic
//! [`Instant`]s (PLAN §5.3).
//!
//! The first observed frame anchors the clock: its media time corresponds
//! to its arrival instant. Every later frame is observed too; if one
//! arrives *earlier* than the clock predicts (the first frames were slowed
//! by encoder start-up), the anchor moves earlier so no frame is ever
//! scheduled before it existed. It never moves later: late frames are the
//! sinks' business (playout delay, dropping).
//!
//! Media time is shared by all tracks (video pts at 90 kHz and audio pts at
//! their sample rate both start near zero on one timeline, see
//! `media::source`), so one clock serves video and audio.
//!
//! Sources: PLAN §5.3.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::util::time::ticks_to_duration;

/// Anchor moves smaller than this are ignored (scheduling jitter).
const REANCHOR_THRESHOLD: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy)]
struct Anchor {
    at: Instant,
    media: Duration,
}

#[derive(Debug, Default)]
pub struct MediaClock {
    anchor: Mutex<Option<Anchor>>,
}

impl MediaClock {
    pub fn new() -> MediaClock {
        MediaClock::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Anchor>> {
        // A poisoned lock only means a panic elsewhere; the data is a Copy.
        self.anchor.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records that the frame with `pts` (at `rate` Hz) arrived now.
    pub fn observe(&self, pts: u64, rate: u32) {
        self.observe_at(pts, rate, Instant::now());
    }

    /// Like [`MediaClock::observe`] with an explicit arrival time.
    pub fn observe_at(&self, pts: u64, rate: u32, now: Instant) {
        let media = ticks_to_duration(pts, rate);
        let mut a = self.lock();
        match *a {
            None => *a = Some(Anchor { at: now, media }),
            Some(old) => {
                // When the clock says this frame is due.
                let due = instant_for(old, media);
                if let Some(early) = due.checked_duration_since(now) {
                    if early > REANCHOR_THRESHOLD {
                        debug!(
                            "media clock: frame {} ms early, re-anchoring",
                            early.as_millis()
                        );
                        *a = Some(Anchor { at: now, media });
                    }
                }
            }
        }
    }

    /// Moves the anchor so that `pts` happens at `now`, also later (a sink
    /// that fell behind: a stalled encoder or a jump in the input).
    pub fn reanchor_at(&self, pts: u64, rate: u32, now: Instant) {
        let media = ticks_to_duration(pts, rate);
        *self.lock() = Some(Anchor { at: now, media });
    }

    pub fn is_anchored(&self) -> bool {
        self.lock().is_some()
    }

    /// The instant at which media time `pts / rate` happens (before any
    /// playout delay). `None` until the first frame is observed.
    pub fn instant_for(&self, pts: u64, rate: u32) -> Option<Instant> {
        let media = ticks_to_duration(pts, rate);
        self.lock().map(|a| instant_for(a, media))
    }

    /// The media time at `now`: where the stream "is" right now. `None`
    /// until anchored; zero before the anchor's media time.
    pub fn media_time_at(&self, now: Instant) -> Option<Duration> {
        self.lock().map(|a| {
            if now >= a.at {
                a.media + (now - a.at)
            } else {
                a.media.saturating_sub(a.at - now)
            }
        })
    }

    /// How late a frame is at `now` relative to when it was due (negative
    /// when early), in nanoseconds. `None` until anchored.
    pub fn lateness(&self, pts: u64, rate: u32, now: Instant) -> Option<i128> {
        let due = self.instant_for(pts, rate)?;
        Some(if now >= due {
            (now - due).as_nanos() as i128
        } else {
            -((due - now).as_nanos() as i128)
        })
    }
}

fn instant_for(a: Anchor, media: Duration) -> Instant {
    if media >= a.media {
        a.at + (media - a.media)
    } else {
        // Before the anchor (e.g. audio that starts a bit before video).
        a.at.checked_sub(a.media - media).unwrap_or(a.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_on_first_frame() {
        let c = MediaClock::new();
        let t0 = Instant::now();
        assert!(c.instant_for(0, 90_000).is_none());
        c.observe_at(9_000, 90_000, t0); // 100 ms media time
        assert_eq!(c.instant_for(9_000, 90_000), Some(t0));
        assert_eq!(
            c.instant_for(18_000, 90_000),
            Some(t0 + Duration::from_millis(100))
        );
        // Audio at 48 kHz on the same timeline.
        assert_eq!(
            c.instant_for(9_600, 48_000),
            Some(t0 + Duration::from_millis(100))
        );
        assert_eq!(
            c.instant_for(0, 48_000),
            Some(t0 - Duration::from_millis(100))
        );
        assert_eq!(
            c.media_time_at(t0 + Duration::from_millis(50)),
            Some(Duration::from_millis(150))
        );
    }

    #[test]
    fn moves_earlier_never_later() {
        let c = MediaClock::new();
        let t0 = Instant::now();
        // First frame was 40 ms slow (encoder start-up).
        c.observe_at(0, 90_000, t0 + Duration::from_millis(40));
        // A late frame does not move the anchor.
        c.observe_at(3_000, 90_000, t0 + Duration::from_millis(200));
        assert_eq!(
            c.instant_for(0, 90_000),
            Some(t0 + Duration::from_millis(40))
        );
        assert_eq!(
            c.lateness(3_000, 90_000, t0 + Duration::from_millis(200)),
            Some(126_666_667) // 200 ms - (40 ms + 33.333333 ms)
        );
        // Frame at media 1 s arrives at t0 + 1.01 s: 30 ms early, re-anchor.
        c.observe_at(90_000, 90_000, t0 + Duration::from_millis(1010));
        assert_eq!(
            c.instant_for(0, 90_000),
            Some(t0 + Duration::from_millis(10))
        );
        // Sub-millisecond jitter is ignored.
        c.observe_at(180_000, 90_000, t0 + Duration::from_micros(2_009_500));
        assert_eq!(
            c.instant_for(0, 90_000),
            Some(t0 + Duration::from_millis(10))
        );
        assert!(c.lateness(0, 90_000, t0).unwrap() < 0);
        // An explicit re-anchor may move it later.
        c.reanchor_at(0, 90_000, t0 + Duration::from_secs(5));
        assert_eq!(
            c.instant_for(90_000, 90_000),
            Some(t0 + Duration::from_secs(6))
        );
    }
}
