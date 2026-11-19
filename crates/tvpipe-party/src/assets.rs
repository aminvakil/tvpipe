//! Pages embedded in the binary ([`assets!`](crate::assets)) and how they
//! are served: GET/HEAD only, no listings, no dot files or `..`, a fixed
//! MIME table, ETags, `Cache-Control: no-cache`, `nosniff`, and a CSP for
//! HTML (docs/party-design.md §4).

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

/// One embedded file.
#[derive(Debug)]
pub struct Asset {
    path: &'static str,
    bytes: &'static [u8],
    hash: u64,
}

/// FNV-1a, 64 bits: the embedded files' ETags (computed at compile time).
const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

impl Asset {
    #[doc(hidden)]
    pub const fn new(path: &'static str, bytes: &'static [u8]) -> Asset {
        Asset {
            path,
            bytes,
            hash: fnv1a(bytes),
        }
    }

    /// The path below the directory (`index.html`, `fonts/a.woff2`).
    pub fn path(&self) -> &'static str {
        self.path
    }

    /// The contents.
    pub fn bytes(&self) -> &'static [u8] {
        self.bytes
    }

    fn etag(&self) -> String {
        format!("W/\"{:016x}-{:x}\"", self.hash, self.bytes.len())
    }
}

/// A directory of files embedded with [`assets!`](crate::assets).
///
/// With the environment variable `TVPIPE_PARTY_DEV` set (not empty, not
/// `0`), files are read from the source directory instead, so pages can
/// be edited without recompiling.
#[derive(Debug)]
pub struct Assets {
    source: &'static str,
    files: &'static [Asset],
}

impl Assets {
    #[doc(hidden)]
    pub const fn new(source: &'static str, files: &'static [Asset]) -> Assets {
        Assets { source, files }
    }

    /// The embedded files.
    pub fn files(&self) -> &'static [Asset] {
        self.files
    }

    /// The directory the files were embedded from (at build time).
    pub fn source_dir(&self) -> &'static str {
        self.source
    }

    /// The embedded file at `path` (no leading `/`).
    pub fn get(&self, path: &str) -> Option<&'static Asset> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Compares the embedded list with the source directory: `Err` lists
    /// files on disk that are not embedded (dot files ignored) and
    /// embedded ones that are gone. For a test:
    /// `assert!(PHONE.check_dir().is_ok())`.
    pub fn check_dir(&self) -> Result<(), String> {
        let mut on_disk = Vec::new();
        list(Path::new(self.source), "", &mut on_disk)
            .map_err(|e| format!("{}: {e}", self.source))?;
        let missing: Vec<&str> = on_disk
            .iter()
            .map(String::as_str)
            .filter(|p| self.get(p).is_none())
            .collect();
        let gone: Vec<&str> = self
            .files
            .iter()
            .map(|f| f.path)
            .filter(|p| !on_disk.iter().any(|d| d == p))
            .collect();
        if missing.is_empty() && gone.is_empty() {
            return Ok(());
        }
        Err(format!(
            "{}: not embedded: {missing:?}; embedded but not on disk: {gone:?}",
            self.source
        ))
    }
}

fn list(dir: &Path, prefix: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = format!("{prefix}{name}");
        if entry.file_type()?.is_dir() {
            list(&entry.path(), &format!("{rel}/"), out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Embeds files of a directory (relative to the crate's `Cargo.toml`) in
/// the binary, as a `static` [`Assets`](crate::Assets):
///
/// ```ignore
/// static PHONE: tvpipe_party::Assets =
///     tvpipe_party::assets!("web/phone", ["index.html", "phone.js", "style.css"]);
/// ```
///
/// The list is explicit (no build script); [`Assets::check_dir`] in a test
/// catches a forgotten file.
#[macro_export]
macro_rules! assets {
    ($dir:literal, [$($file:literal),* $(,)?]) => {
        $crate::Assets::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir),
            &[$($crate::Asset::new(
                $file,
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $file)),
            )),*],
        )
    };
}

/// Whether pages come from disk (`TVPIPE_PARTY_DEV`).
fn dev_mode() -> bool {
    static DEV: OnceLock<bool> = OnceLock::new();
    *DEV.get_or_init(|| std::env::var("TVPIPE_PARTY_DEV").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// What a request for a file gets.
pub(crate) enum Lookup {
    File {
        bytes: Cow<'static, [u8]>,
        etag: String,
        mime: &'static str,
    },
    /// `/dir` → `/dir/`.
    Redirect,
    NotFound,
    BadRequest,
}

/// The file for a decoded request path below a directory (`path` starts
/// with `/`).
pub(crate) fn lookup(assets: &Assets, raw_path: &str) -> Lookup {
    let Some(rel) = percent_decode(raw_path) else {
        return Lookup::BadRequest;
    };
    let rel = rel.strip_prefix('/').unwrap_or(&rel).to_owned();
    if rel.contains('\0')
        || rel.contains('\\')
        || rel
            .split('/')
            .any(|p| p == ".." || p == "." || p.starts_with('.'))
    {
        return Lookup::NotFound;
    }
    let file = if rel.is_empty() || rel.ends_with('/') {
        format!("{rel}index.html")
    } else {
        rel.clone()
    };
    if dev_mode() {
        let base = Path::new(assets.source);
        let p = base.join(&file);
        if p.is_dir() {
            return Lookup::Redirect;
        }
        return match std::fs::read(&p) {
            Ok(bytes) => Lookup::File {
                etag: etag_of(&bytes),
                bytes: Cow::Owned(bytes),
                mime: mime(&file),
            },
            Err(_) => Lookup::NotFound,
        };
    }
    if let Some(a) = assets.get(&file) {
        return Lookup::File {
            bytes: Cow::Borrowed(a.bytes),
            etag: a.etag(),
            mime: mime(&file),
        };
    }
    // A directory: some file below it exists.
    let dir = format!("{file}/");
    if !file.is_empty() && assets.files.iter().any(|f| f.path.starts_with(&dir)) {
        return Lookup::Redirect;
    }
    Lookup::NotFound
}

/// A weak ETag from the contents.
pub(crate) fn etag_of(bytes: &[u8]) -> String {
    format!("W/\"{:016x}-{:x}\"", fnv1a(bytes), bytes.len())
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The MIME type by extension (`@tvpipe/party`'s table).
pub(crate) fn mime(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FILES: Assets = Assets::new(
        "/nonexistent",
        &[
            Asset::new("index.html", b"<!doctype html>"),
            Asset::new("app.js", b"export {}"),
            Asset::new("sub/index.html", b"sub"),
            Asset::new("fonts/a.woff2", b"font"),
        ],
    );

    fn found(path: &str) -> Option<(Vec<u8>, &'static str)> {
        match lookup(&FILES, path) {
            Lookup::File { bytes, mime, .. } => Some((bytes.into_owned(), mime)),
            _ => None,
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(
            found("/").unwrap(),
            (b"<!doctype html>".to_vec(), "text/html; charset=utf-8")
        );
        assert_eq!(
            found("/app.js").unwrap().1,
            "text/javascript; charset=utf-8"
        );
        assert_eq!(found("/sub/").unwrap().0, b"sub");
        assert_eq!(found("/fonts/a%2ewoff2").unwrap().1, "font/woff2");
        assert!(matches!(lookup(&FILES, "/sub"), Lookup::Redirect));
        assert!(matches!(lookup(&FILES, "/fonts"), Lookup::Redirect));
        for bad in [
            "/../x",
            "/sub/../index.html",
            "/.secret",
            "/a%00",
            "/a\\b",
            "/nope",
        ] {
            assert!(matches!(lookup(&FILES, bad), Lookup::NotFound), "{bad}");
        }
        assert!(matches!(lookup(&FILES, "/%zz"), Lookup::BadRequest));
        assert!(matches!(lookup(&FILES, "/%ff"), Lookup::BadRequest));
        assert!(etag_of(b"x").starts_with("W/\""));
    }
}
