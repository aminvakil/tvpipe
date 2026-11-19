//! A small software 2D renderer for RGBA frames (docs/multiscreen-design.md
//! §5): what a game's TV picture needs, without dependencies.
//!
//! - [`Canvas`]: an RGBA8 buffer with a clip rectangle; rectangles,
//!   rounded rectangles, circles, any [`Path`] filled with exact
//!   anti-aliasing (a signed-area coverage accumulator, non-zero winding),
//!   round-capped strokes, images (scaled bilinearly) and QR codes.
//! - [`Font`]: TrueType (`glyf`) fonts: character maps, metrics, composite
//!   glyphs, kerning (`kern` or GPOS pairs). No hinting, no shaping, no
//!   CFF (`.otf`).
//! - [`Text`]: layout, measuring, alignment, wrapping, ellipsis, font
//!   fallback and a glyph cache. Emoji that no font has are left out.
//!
//! ```
//! use tvpipe_canvas::{Canvas, Color, Path};
//!
//! let mut canvas = Canvas::new(320, 180);
//! canvas.clear(Color::hex(0x1d1d1f));
//! canvas.fill_rounded_rect(20.0, 20.0, 280.0, 60.0, 12.0, Color::WHITE);
//! let mut p = Path::new();
//! p.move_to(40.0, 140.0).quad_to(160.0, 60.0, 280.0, 140.0);
//! canvas.stroke_path(&p, 6.0, Color::hex(0xff4f6d));
//! assert_eq!(canvas.pixel(160, 50), Color::WHITE);
//! assert_eq!(canvas.data().len(), 320 * 180 * 4);
//! ```

mod canvas;
mod font;
mod path;
mod raster;
mod stroke;
mod text;

pub use canvas::{Canvas, Color};
pub use font::{Font, FontError};
pub use path::Path;
pub use raster::IRect;
pub use text::{is_emoji, strip_emoji, Align, Baseline, Text, TextStyle};
