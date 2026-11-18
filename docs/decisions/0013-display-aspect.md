# 0013: `Capabilities::display_aspect` (amends 0001)

Status: accepted by lead (2026-11-18). Proposed by W12.

Receivers stretch what they get to fill their screen: a projector on
Cast showed a 16:10 browser share (1728x1080) stretched to 16:9. Senders
that choose their own frame size (`tvpipe share`, later the headless
browser) must produce frames of the screen's shape and pad the picture.

## Interface change

`src/types.rs`, a field on the frozen `Capabilities`:

```rust
/// The shape of the receiver's screen, reduced (e.g. `(16, 9)`), if it
/// says: it stretches pictures of another shape to fill it.
pub display_aspect: Option<(u32, u32)>,
```

plus `types::aspect_of(w, h)` (reduces a size to its ratio).

- Cast: the ANSWER's `display.aspectRatio`, else the shape of
  `display.dimensions`, else `None` (openscreen answer fields).
- AirPlay mirroring: the canvas from SETUP.
- Everything else: `None`. Users of the field treat `None` as 16:9.

`max_width`/`max_height` stay limits, not the screen: for Cast they are
the offer narrowed by the ANSWER's constraints, which can have another
shape than the display.

## Related

- `media::h264::Sps` gains `sar` (VUI sample aspect ratio, H.264 Table
  E-1); `share` warns when a browser's stream says pixels are not square.
- How `share` uses it (shape choice with several receivers, sizes,
  centering): `docs/share-protocol.md`, "Stream size and shape";
  `src/browser/layout.rs`.
