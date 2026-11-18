//! Core types shared by every workstream (PLAN §3).
//!
//! **Frozen in M0.** Changing anything public here needs a proposal in
//! `docs/decisions/` approved by the lead. Adding helper methods is fine.

use std::sync::Arc;

use crate::error::Result;
use crate::util::ratio::Ratio;

/// Video timestamps are always in this clock rate.
pub const VIDEO_CLOCK_RATE: u32 = 90_000;

/// Which audio encode a track belongs to. AirPlay and Cast want different
/// audio codecs, so ffmpeg produces one audio track per family in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioFamily {
    AirPlay,
    Cast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Video,
    Audio(AudioFamily),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    /// Apple Lossless.
    Alac,
    /// AAC-LC (raw access units, no ADTS).
    AacLc,
    /// AAC-ELD (AirPlay; needs libfdk_aac in ffmpeg).
    AacEld,
    Opus,
    /// Signed 16-bit little-endian interleaved PCM.
    PcmS16le,
}

/// One encoded access unit as it leaves the demuxer.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub track: TrackKind,
    /// Presentation timestamp: [`VIDEO_CLOCK_RATE`] ticks for video, sample
    /// ticks (`AudioInfo::sample_rate`) for audio. Starts near zero and is
    /// monotonic per track (no B-frames, so pts == dts for video).
    pub pts: u64,
    /// Video: IDR frame. Audio: always `true`.
    pub keyframe: bool,
    /// Video: AVCC (4-byte big-endian length-prefixed NAL units, no start
    /// codes, no in-band SPS/PPS required). Audio: one raw codec frame.
    pub data: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInfo {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: Ratio,
    /// The `avcC` (AVCDecoderConfigurationRecord) or `hvcC` record.
    pub avcc: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u8,
    /// Samples per channel per frame (e.g. 352 for AirPlay ALAC, 480 for
    /// 10 ms Opus at 48 kHz).
    pub frame_samples: u32,
    /// Codec configuration (ALAC magic cookie, AudioSpecificConfig, Opus
    /// header); empty if the codec has none.
    pub config: Vec<u8>,
}

/// An audio format a receiver accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u8,
}

/// What a connected receiver can take. Filled in by the sink after its
/// connect/negotiation phase, before ffmpeg starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: Ratio,
    /// Preferred first.
    pub video_codecs: Vec<VideoCodec>,
    /// Preferred first. Empty means the receiver takes no audio.
    pub audio_formats: Vec<AudioFormat>,
    /// Which audio encode this receiver consumes.
    pub audio_family: AudioFamily,
    pub max_bitrate_kbps: Option<u32>,
    /// The lowest playout delay the receiver accepts, if it says.
    pub min_latency_ms: Option<u32>,
    /// The playout delay the receiver prefers, if it says.
    pub preferred_latency_ms: Option<u32>,
}

/// Which encoder makes the video: one of ffmpeg's, or our own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Encoder {
    /// Probe `ffmpeg -encoders` and pick the best available (the built-in
    /// encoder when raw input has no ffmpeg; decision 0006).
    #[default]
    Auto,
    /// `media::h264enc` in-process (raw SLRW input and the test pattern;
    /// decision 0006). Never handed to ffmpeg.
    Builtin,
    X264,
    VideoToolbox,
    Nvenc,
    Qsv,
    Vaapi,
    MediaFoundation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPlan {
    pub codec: VideoCodec,
    pub encoder: Encoder,
    pub width: u32,
    pub height: u32,
    pub fps: Ratio,
    pub bitrate_kbps: u32,
    /// Keyframe interval in frames.
    pub gop: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPlan {
    pub family: AudioFamily,
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_samples: u32,
    pub bitrate_kbps: Option<u32>,
}

/// What ffmpeg must produce. Built by `session.rs` from the intersection of
/// all sinks' [`Capabilities`] and the user's options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodePlan {
    pub video: VideoPlan,
    /// At most one entry per [`AudioFamily`]; empty for `--no-audio`.
    pub audio: Vec<AudioPlan>,
    /// Target playout delay shared by all sinks.
    pub latency_ms: u32,
}

/// Result of [`Sink::push`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkStatus {
    /// The frame was queued or sent.
    Accepted,
    /// The frame was dropped (congestion, or waiting for a keyframe). Not an
    /// error; the fanout keeps going.
    Dropped,
    /// The receiver ended the session (user stopped mirroring on the TV).
    /// The session removes the sink.
    Finished,
}

/// A receiver we stream to. One implementation per protocol family.
///
/// Lifecycle: construct + connect (protocol-specific, returns the sink) →
/// `capabilities()` → `start()` once ffmpeg's stream info is known →
/// `push()` for every frame → `stop()`.
pub trait Sink: Send {
    /// Human-readable receiver name, for logs.
    fn name(&self) -> &str;

    /// Valid after connect.
    fn capabilities(&self) -> &Capabilities;

    /// Sets up the media streams. `audio` is the track of this sink's
    /// [`AudioFamily`], if any.
    fn start(&mut self, video: &VideoInfo, audio: Option<&AudioInfo>) -> Result<()>;

    /// Hands over one frame. Must not block for long: queue internally and
    /// return [`SinkStatus::Dropped`] when full.
    fn push(&mut self, frame: &EncodedFrame) -> Result<SinkStatus>;

    /// True when the receiver asked for a keyframe (PLI, loss, join). The
    /// fanout then drops video for this sink until the next keyframe.
    fn wants_keyframe(&self) -> bool;

    /// Tears the session down so the TV leaves mirroring mode. Must be safe
    /// to call more than once.
    fn stop(&mut self) -> Result<()>;
}

impl Capabilities {
    /// Whether this receiver accepts a given audio format.
    pub fn accepts_audio(&self, fmt: &AudioFormat) -> bool {
        self.audio_formats.contains(fmt)
    }
}
