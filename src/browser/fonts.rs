//! The fonts of tvpipe's pages, compiled in so the pages look the same
//! without a network connection: Sixtyfour (the wordmark), Michroma
//! (labels), IBM Plex Sans (text) and IBM Plex Mono (numbers). Latin
//! subsets as Google Fonts serves them, SIL Open Font License 1.1
//! (`LICENSES/OFL-1.1-fonts.txt`). The website uses the same files.

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

/// The font served at `/fonts/NAME`, if there is one.
pub fn get(path: &str) -> Option<&'static [u8]> {
    let name = path.strip_prefix("/fonts/")?;
    FONTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, data)| *data)
}

/// The reply for a `/fonts/` path: the font, cached for a day (the files
/// only change with tvpipe), or 404.
pub fn reply(path: &str) -> Reply {
    match get(path) {
        Some(data) => {
            let mut r = httpd::response(200, "OK", "font/woff2", data.to_vec());
            r.headers.set("Cache-Control", "max-age=86400");
            Reply::Response(r)
        }
        None => Reply::Response(httpd::error_response(404, "Not Found")),
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
    }
}
