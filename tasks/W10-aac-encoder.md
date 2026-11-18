# W10 AAC-LC encoder (option C, audio)

Status: in progress. Owns `src/media/aacenc/**`, `tests/vectors/aacenc/**`.

API (leaf module):
`aacenc::Encoder::new(sample_rate, channels, bitrate_bps) -> Result`,
`encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>>` (exactly 1024 samples
per channel, interleaved; returns one raw AAC access unit, no ADTS),
`flush()`, `audio_specific_config() -> Vec<u8>`, `delay_samples()`.
Optional `adts()` wrapper for testing.

- [ ] MDCT (sine/KBD windows, 1024/128, long/start/short/stop window
      sequences; a simple transient detector), scalefactor bands per
      sample rate (44.1/48 kHz at least), quantization with a simple
      bit-allocation/rate loop (no full psychoacoustic model needed; a
      basic masking estimate is a plus), Huffman codebooks (spectral +
      scalefactor tables from the standard), section data, M/S stereo
      (optional), bit reservoir for CBR-ish output.
- [ ] Tests: decode our output with ffmpeg (run as an oracle) and check
      SNR on sine/chirp/noise/music-like signals at 96–256 kbit/s; check
      bitrate; ffprobe accepts the stream; no-panic fuzz on encoder inputs.
- [ ] Speed: well under real time on one core (report ×realtime).

Clean room: ISO/IEC 14496-3 / 13818-7 descriptions, papers, Wikipedia;
no FAAC, ffmpeg aacenc, fdk-aac source (see decision 0006).
