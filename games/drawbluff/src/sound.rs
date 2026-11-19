//! The TV's sound: short synthesized cues, as the web screen plays them
//! (`beep` in `web/screen/screen.js`: triangle notes with a quick attack
//! and an exponential decay), mixed into the stream's continuous audio.

use std::collections::VecDeque;

use tvpipe_party::Value;

/// Samples per second of the stream's audio.
pub const RATE: u32 = 48_000;
/// Channels of the stream's audio.
pub const CHANNELS: u8 = 2;

/// Notes played one after the other (`len` seconds each, overlapping a
/// little), mono, -1.0..1.0.
pub fn beep(notes: &[f32], len: f32) -> Vec<f32> {
    let rate = RATE as f32;
    let step = len * 0.9;
    let total = step * notes.len().saturating_sub(1) as f32 + len + 0.02;
    let mut out = vec![0.0f32; (total * rate).ceil() as usize + 1];
    for (i, &f) in notes.iter().enumerate() {
        let start = (i as f32 * step * rate) as usize;
        let n = ((len + 0.02) * rate) as usize;
        for k in 0..n {
            let t = k as f32 / rate;
            // 0.0001 → 0.25 in 10 ms, then down to 0.0001 at `len`.
            let gain = if t < 0.01 {
                0.0001 * (2500.0f32).powf(t / 0.01)
            } else if t < len {
                0.25 * (0.0004f32).powf((t - 0.01) / (len - 0.01))
            } else {
                0.0001
            };
            let phase = (t * f).fract();
            let tri = 4.0 * (phase - 0.5).abs() - 1.0;
            if let Some(s) = out.get_mut(start + k) {
                *s += tri * gain;
            }
        }
    }
    out
}

/// Which cue a view change calls for (the web screen's rules): a phase
/// that wants input, the podium, a player joining the lobby, a reveal
/// step (the truth or a lie).
#[derive(Debug, Default)]
pub struct Cues {
    phase: String,
    step: u64,
    lobby: usize,
}

impl Cues {
    /// The sound for this screen view, if it changed something that has one.
    pub fn update(&mut self, view: &Value) -> Option<Vec<f32>> {
        let s = &view["screen"];
        let phase = s["phase"].as_str().unwrap_or_default();
        let mut cue = None;
        if phase != self.phase {
            cue = match phase {
                "draw" | "lie" | "vote" => Some(beep(&[523.0, 659.0, 784.0], 0.12)),
                "final" => Some(beep(&[523.0, 659.0, 784.0, 1047.0], 0.18)),
                _ => None,
            };
            phase.clone_into(&mut self.phase);
            self.step = 0;
        }
        if phase == "lobby" {
            let n = view["roster"].as_array().map_or(0, <[Value]>::len);
            if n > self.lobby {
                cue = Some(beep(&[660.0, 880.0], 0.12));
            }
            self.lobby = n;
        }
        if phase == "reveal" {
            let step = s["step"].as_u64().unwrap_or(0);
            if step > self.step {
                let truth = step
                    .checked_sub(1)
                    .and_then(|i| s["items"].as_array()?.get(i as usize))
                    .is_some_and(|it| it["truth"].as_bool() == Some(true));
                cue = Some(if truth {
                    beep(&[784.0, 988.0, 1175.0], 0.12)
                } else {
                    beep(&[392.0, 330.0], 0.12)
                });
                self.step = step;
            }
        }
        cue
    }
}

/// Cues mixed over silence, taken out as the stream's PCM.
#[derive(Debug, Default)]
pub struct Mixer {
    pending: VecDeque<f32>,
}

impl Mixer {
    /// Adds a sound (mono) from now on, over what is still playing.
    pub fn play(&mut self, sound: &[f32]) {
        for (i, &s) in sound.iter().enumerate() {
            match self.pending.get_mut(i) {
                Some(p) => *p += s,
                None => self.pending.push_back(s),
            }
        }
    }

    /// The next `frames` sample frames, interleaved stereo 16-bit
    /// (silence when nothing plays).
    pub fn take(&mut self, frames: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(frames * CHANNELS as usize);
        for _ in 0..frames {
            let s = self.pending.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
            let v = (s * 32767.0) as i16;
            out.extend(std::iter::repeat_n(v, CHANNELS as usize));
        }
        out
    }

    /// Whether a sound is still playing.
    pub fn playing(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tvpipe_party::json;

    #[test]
    fn beeps_are_short_and_quiet() {
        let b = beep(&[523.0, 659.0, 784.0], 0.12);
        let secs = b.len() as f32 / RATE as f32;
        assert!((0.35..0.37).contains(&secs), "{secs}");
        let peak = b.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((0.2..0.5).contains(&peak), "{peak}");
    }

    #[test]
    fn cues_follow_the_web_screen() {
        let mut c = Cues::default();
        let lobby =
            |n: usize| json!({ "screen": { "phase": "lobby" }, "roster": vec![json!({}); n] });
        assert!(c.update(&lobby(1)).is_some(), "a player joined");
        assert!(c.update(&lobby(1)).is_none());
        assert!(c.update(&lobby(0)).is_none(), "one left");
        assert!(c
            .update(&json!({ "screen": { "phase": "draw" } }))
            .is_some());
        assert!(c
            .update(&json!({ "screen": { "phase": "draw", "done": ["p1"] } }))
            .is_none());
        let reveal = |step: u64| json!({ "screen": { "phase": "reveal", "step": step, "items": [{ "truth": false }, { "truth": true }] } });
        assert!(c.update(&reveal(0)).is_none());
        assert!(c.update(&reveal(1)).is_some());
        assert!(c.update(&reveal(1)).is_none());
        assert!(c.update(&reveal(2)).is_some());
        assert!(c
            .update(&json!({ "screen": { "phase": "final" } }))
            .is_some());
    }

    #[test]
    fn the_mixer_mixes_and_pads_with_silence() {
        let mut m = Mixer::default();
        m.play(&[0.5, 0.5]);
        m.play(&[0.25]);
        assert_eq!(m.take(3), vec![24575, 24575, 16383, 16383, 0, 0]);
        assert!(!m.playing());
    }
}
