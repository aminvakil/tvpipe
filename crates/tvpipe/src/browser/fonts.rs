//! The fonts of tvpipe's pages, compiled in so the pages look the same
//! without a network connection: Sixtyfour (the wordmark), Michroma
//! (labels), IBM Plex Sans (text) and IBM Plex Mono (numbers). Latin
//! subsets as Google Fonts serves them, SIL Open Font License 1.1
//! (`LICENSES/OFL-1.1-fonts.txt`). The website uses the same files.
//!
//! `ui.css` is tvpipe's look for pages that link a stylesheet (the
//! playshare launcher and game pages, at `/_tvpipe/ui.css`): the tokens,
//! these fonts (relative URLs: `fonts/` beside the stylesheet) and the
//! parts of the share page.

use crate::net::http::Response;
use crate::net::httpd::{self, Reply};

const FONTS: &[(&str, &[u8])] = &[
    ("sixtyfour.woff2", include_bytes!("fonts/sixtyfour.woff2")),
    ("michroma.woff2", include_bytes!("fonts/michroma.woff2")),
    ("plex-sans.woff2", include_bytes!("fonts/plex-sans.woff2")),
    (
        "plex-mono-400.woff2",
        include_bytes!("fonts/plex-mono-400.woff2"),
    ),
    (
        "plex-mono-500.woff2",
        include_bytes!("fonts/plex-mono-500.woff2"),
    ),
];

/// tvpipe's stylesheet (see the module documentation).
pub const UI_CSS: &str = include_str!("ui.css");

/// The font named `name` (`michroma.woff2`), if there is one.
pub fn font(name: &str) -> Option<&'static [u8]> {
    FONTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, data)| *data)
}

/// The font served at `/fonts/NAME`, if there is one.
pub fn get(path: &str) -> Option<&'static [u8]> {
    font(path.strip_prefix("/fonts/")?)
}

/// The reply for a `/fonts/` path: the font, cached for a day (the files
/// only change with tvpipe), or 404.
pub fn reply(path: &str) -> Reply {
    Reply::Response(font_response(get(path)))
}

/// The response for a font (`None`: 404), cached for a day.
pub fn font_response(font: Option<&'static [u8]>) -> Response {
    match font {
        Some(data) => {
            let mut r = httpd::response(200, "OK", "font/woff2", data.to_vec());
            r.headers.set("Cache-Control", "max-age=86400");
            r
        }
        None => httpd::error_response(404, "Not Found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_every_font_and_nothing_else() {
        for (name, data) in FONTS {
            assert_eq!(&data[..4], b"wOF2", "{name}");
            assert_eq!(get(&format!("/fonts/{name}")), Some(*data));
        }
        assert_eq!(get("/fonts/"), None);
        assert_eq!(get("/fonts/../share.html"), None);
        assert_eq!(get("/sixtyfour.woff2"), None);
        assert_eq!(font("../fonts.rs"), None);
    }

    #[test]
    fn the_stylesheet_names_only_fonts_there_are() {
        let mut n = 0;
        for part in UI_CSS.split("url(").skip(1) {
            let url = part.split(')').next().unwrap();
            let name = url.strip_prefix("fonts/").expect(url);
            assert!(font(name).is_some(), "{url}");
            n += 1;
        }
        assert_eq!(n, FONTS.len());
    }
}
