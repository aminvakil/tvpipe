//! `tvpipe game check` and `game pack`: checks a game (a folder or a zip) the way playshare
//! will load and serve it, and packs a folder into a deterministic zip
//! (docs/games.md).
//!
//! What is checked, as errors (the game would not work) or warnings
//! (it may not, somewhere):
//!
//! - `tvpipe.json`: what playshare itself refuses ([`Manifest::parse`]),
//!   unknown fields (typos), the `tvpipe` version the game is made for;
//! - the files: `root` exists, names that are not served (hidden,
//!   backslashes, not UTF-8), unsafe or duplicate names in a zip,
//!   symlinks that leave `root`, names that differ only in case, the
//!   limits of a zip bundle (entries, unpacked size), very large files;
//! - the entry pages exist and are HTML; what they load, followed through
//!   HTML, CSS and JS modules ([`super::scan`]): missing files, files
//!   whose name differs in case, TypeScript left unbuilt, bare imports
//!   no import map resolves, `/_tvpipe/` paths that do not exist, files
//!   from the internet (games should work offline);
//! - `@tvpipe/party`: host pages load the library (`/_tvpipe/lib/`),
//!   phone and TV pages its client; copies bundled with the game.
//!
//! Finally the bundle is opened the way `tvpipe game play` opens it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::bundle::{clean_path, percent_decode, Bundle, Manifest, MANIFEST};
use super::scan::{self, Kind, Ref, RefKind};
use super::web;
use crate::browser::fonts;
use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::util::zip;

/// Files in a bundle at most (the zip reader's limit, the manifest
/// included).
pub const MAX_FILES: usize = 10_000;
/// Unpacked bytes at most (the zip reader's limit).
pub const MAX_TOTAL: u64 = 256 << 20;
/// A file larger than this is worth a warning.
pub const BIG_FILE: u64 = 32 << 20;
/// The tvpipe this is.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Top-level manifest fields playshare knows.
const KNOWN: &[&str] = &[
    "name",
    "title",
    "root",
    "pages",
    "players",
    "tv",
    "limits",
    "tvpipe",
    "version",
    "description",
    "$schema",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
}

/// One finding: where (`file` or `file:line`, empty for the bundle as a
/// whole) and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub level: Level,
    pub at: String,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        if self.at.is_empty() {
            write!(f, "{level}: {}", self.message)
        } else {
            write!(f, "{level}: {}: {}", self.at, self.message)
        }
    }
}

/// A checked game: what was found, and its files for packing.
#[derive(Debug)]
pub struct Checked {
    /// What it was read from.
    pub origin: PathBuf,
    pub is_zip: bool,
    pub manifest: Option<Manifest>,
    /// The manifest as written (unknown fields kept).
    manifest_json: Option<Value>,
    /// The served files: below `root`, by clean path.
    files: BTreeMap<String, Vec<u8>>,
    pub problems: Vec<Problem>,
    /// Hidden and skipped names (never served, never packed).
    pub skipped: usize,
    /// A folder's served directory.
    root_dir: Option<PathBuf>,
    /// Lines for the report that are not problems.
    notes: Vec<String>,
}

impl Checked {
    pub fn errors(&self) -> usize {
        self.problems
            .iter()
            .filter(|p| p.level == Level::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.problems.len() - self.errors()
    }

    /// Files and bytes that are served.
    pub fn size(&self) -> (usize, u64) {
        (
            self.files.len(),
            self.files.values().map(|f| f.len() as u64).sum(),
        )
    }

    /// The served paths, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Adds a line to the report.
    pub fn note(&mut self, line: String) {
        self.notes.push(line);
    }

    /// Leaves out a file (the zip being written, when it is below the
    /// root): true if it was there or would be.
    pub fn exclude(&mut self, path: &Path) -> bool {
        let Some(root) = &self.root_dir else {
            return false;
        };
        let abs = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => {
                let parent = match path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => std::fs::canonicalize(p),
                    _ => std::env::current_dir(),
                };
                match (parent, path.file_name()) {
                    (Ok(p), Some(n)) => p.join(n),
                    _ => return false,
                }
            }
        };
        let Ok(rel) = abs.strip_prefix(root) else {
            return false;
        };
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        self.files.remove(&rel.join("/"));
        true
    }

    /// The report: one line per problem, then a summary.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for p in &self.problems {
            out.push_str(&p.to_string());
            out.push('\n');
        }
        for n in &self.notes {
            out.push_str(&format!("note: {n}\n"));
        }
        let (n, bytes) = self.size();
        let what = match &self.manifest {
            Some(m) if m.root.is_empty() => format!("{} ({})", m.title, m.name),
            Some(m) => format!("{} ({}, root {})", m.title, m.name, m.root),
            None => self.origin.display().to_string(),
        };
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        out.push_str(&format!(
            "{what}: {}, {}; {}, {}\n",
            plural(n, "file"),
            human(bytes),
            plural(self.errors(), "error"),
            plural(self.warnings(), "warning"),
        ));
        out
    }

    fn add(&mut self, level: Level, at: impl Into<String>, message: impl Into<String>) {
        let p = Problem {
            level,
            at: at.into(),
            message: message.into(),
        };
        if !self.problems.contains(&p) {
            self.problems.push(p);
        }
    }

    fn error(&mut self, at: impl Into<String>, message: impl Into<String>) {
        self.add(Level::Error, at, message);
    }

    fn warn(&mut self, at: impl Into<String>, message: impl Into<String>) {
        self.add(Level::Warning, at, message);
    }
}

/// `1.4 MiB`.
pub fn human(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1 << 20 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1 << 20) as f64)
    }
}

/// Whether a game made for tvpipe `declared` runs on tvpipe `ours`:
/// the same major version (for 0.x, the same minor) and not newer.
/// `Err` says why not, or that `declared` is not a version.
pub fn version_compatible(declared: &str, ours: &str) -> std::result::Result<(), String> {
    fn parse(v: &str) -> Option<(u32, u32, u32)> {
        let mut it = v.trim().split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next().map_or(Some(0), |m| m.parse().ok())?;
        let patch = it
            .next()
            .map_or(Some(0), |p| p.split(['-', '+']).next()?.parse().ok())?;
        it.next().is_none().then_some((major, minor, patch))
    }
    let d = parse(declared).ok_or_else(|| {
        format!(
            "\"tvpipe\" must be the version the game is made for, like \"{}\", not {declared:?}",
            short_version(ours)
        )
    })?;
    let o = parse(ours).ok_or_else(|| format!("this tvpipe's version {ours:?}"))?;
    let series = |v: (u32, u32, u32)| if v.0 == 0 { (0, v.1) } else { (v.0, 0) };
    if d > o {
        return Err(format!(
            "the game needs tvpipe {declared} or newer; this is {ours}: update tvpipe"
        ));
    }
    if series(d) != series(o) {
        return Err(format!(
            "the game is made for tvpipe {declared}; this tvpipe {ours} serves a newer @tvpipe/party that may not fit: update the game for {} (docs/games.md, Versions) and set \"tvpipe\": \"{}\"",
            short_version(ours),
            short_version(ours)
        ));
    }
    Ok(())
}

/// `0.2.0` → `0.2`.
pub fn short_version(v: &str) -> String {
    v.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// Checks a folder or a `.zip`. `Err` only when `path` cannot be read at
/// all; everything else is a [`Problem`].
pub fn check(path: &Path) -> Result<Checked> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(format!("{}", path.display()), e))?;
    let mut c = Checked {
        origin: path.to_owned(),
        is_zip: !meta.is_dir(),
        manifest: None,
        manifest_json: None,
        files: BTreeMap::new(),
        problems: Vec::new(),
        skipped: 0,
        root_dir: None,
        notes: Vec::new(),
    };
    if meta.is_dir() {
        gather_folder(path, &mut c)?;
    } else {
        let data =
            std::fs::read(path).map_err(|e| Error::io(format!("read {}", path.display()), e))?;
        gather_zip(&data, &mut c);
    }
    if c.manifest.is_some() && !c.files.is_empty() {
        check_files(&mut c);
        check_pages(&mut c);
    }
    // Last: what playshare itself says, if nothing above explains it.
    if c.errors() == 0 {
        if let Err(e) = Bundle::open(path) {
            c.error("", format!("tvpipe game play refuses it: {}", e.message()));
        }
    }
    sort(&mut c.problems);
    Ok(c)
}

fn sort(p: &mut [Problem]) {
    // Errors first, then by place (lines in order), stable otherwise.
    fn key(at: &str) -> (String, u32) {
        match at.rsplit_once(':') {
            Some((f, l)) if l.parse::<u32>().is_ok() => (f.to_owned(), l.parse().unwrap_or(0)),
            _ => (at.to_owned(), 0),
        }
    }
    p.sort_by_key(|a| (a.level, key(&a.at)));
}

/// The manifest's own problems (beyond [`Manifest::parse`]): unknown
/// fields, the version.
fn check_manifest_json(v: &Value, c: &mut Checked) {
    if let Some(o) = v.as_object() {
        for k in o.keys() {
            if !KNOWN.contains(&k) {
                c.warn(MANIFEST, format!("unknown field {k:?} (ignored; a typo?)"));
            }
        }
    }
    if let Some(p) = v.get("pages").and_then(Value::as_object) {
        for k in p.keys() {
            if !["host", "tv", "phone", "launcher"].contains(&k) {
                c.warn(
                    MANIFEST,
                    format!("unknown page \"pages.{k}\" (pages are host, tv, phone and launcher)"),
                );
            }
        }
    }
    match v.get("tvpipe") {
        None | Some(Value::Null) => c.warn(
            MANIFEST,
            format!(
                "no \"tvpipe\" version: add \"tvpipe\": \"{}\" (the tvpipe and @tvpipe/party the game is made for)",
                short_version(VERSION)
            ),
        ),
        Some(Value::String(s)) => {
            if let Err(e) = version_compatible(s, VERSION) {
                c.error(MANIFEST, e);
            }
        }
        Some(_) => c.error(
            MANIFEST,
            format!("\"tvpipe\" must be a string like \"{}\"", short_version(VERSION)),
        ),
    }
}

fn parse_manifest(text: &[u8], c: &mut Checked) {
    if text.len() > 64 * 1024 {
        c.error(MANIFEST, "larger than 64 KiB");
        return;
    }
    let text = match std::str::from_utf8(text) {
        Ok(t) => t,
        Err(_) => {
            c.error(MANIFEST, "not UTF-8");
            return;
        }
    };
    match Manifest::parse(text) {
        Ok(m) => {
            let v = Value::parse(text).unwrap_or(Value::Null);
            check_manifest_json(&v, c);
            c.manifest = Some(m);
            c.manifest_json = Some(v);
        }
        Err(e) => {
            let msg = e.message();
            let msg = msg
                .strip_prefix(&format!("{MANIFEST}: "))
                .unwrap_or(msg)
                .to_owned();
            c.error(MANIFEST, msg);
        }
    }
}

/// Where a name is not one playshare serves: why.
fn name_problem(name: &str) -> Option<&'static str> {
    if name.contains('\\') {
        Some("has a backslash: never served (use / between folders)")
    } else if name.chars().any(char::is_control) {
        Some("has control characters: never served")
    } else {
        None
    }
}

/// Characters Windows refuses in file names.
fn windows_problem(segment: &str) -> bool {
    segment
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        || segment.ends_with('.')
        || segment.ends_with(' ')
}

fn gather_folder(path: &Path, c: &mut Checked) -> Result<()> {
    let dir =
        std::fs::canonicalize(path).map_err(|e| Error::io(format!("{}", path.display()), e))?;
    let mpath = dir.join(MANIFEST);
    let text = match std::fs::read(&mpath) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            c.error(
                "",
                format!(
                    "no {MANIFEST} in {}: a game's folder has one at its top (tvpipe game new DIR makes a starter)",
                    path.display()
                ),
            );
            return Ok(());
        }
        Err(e) => return Err(Error::io(format!("read {}", mpath.display()), e)),
    };
    parse_manifest(&text, c);
    let Some(m) = c.manifest.clone() else {
        return Ok(());
    };
    let root = if m.root.is_empty() {
        dir.clone()
    } else {
        match std::fs::canonicalize(dir.join(&m.root)) {
            Ok(r) if r.is_dir() => r,
            Ok(_) => {
                c.error(MANIFEST, format!("\"root\" {:?} is not a folder", m.root));
                return Ok(());
            }
            Err(_) => {
                c.error(
                    MANIFEST,
                    format!(
                        "\"root\" {:?} does not exist: build the game first (npm run build?)",
                        m.root
                    ),
                );
                return Ok(());
            }
        }
    };
    if !root.starts_with(&dir) {
        c.error(
            MANIFEST,
            format!("\"root\" {:?} leaves the game's folder", m.root),
        );
        return Ok(());
    }
    walk(&root, &root, "", c, &mut Vec::new())?;
    c.root_dir = Some(root);
    if c.files.is_empty() {
        c.error(MANIFEST, format!("\"root\" {:?} has no files", m.root));
    }
    Ok(())
}

fn walk(
    root: &Path,
    dir: &Path,
    prefix: &str,
    c: &mut Checked,
    ancestors: &mut Vec<PathBuf>,
) -> Result<()> {
    if ancestors.len() > 32 {
        c.error(
            prefix.trim_end_matches('/'),
            "folders nested more than 32 deep",
        );
        return Ok(());
    }
    let real =
        std::fs::canonicalize(dir).map_err(|e| Error::io(format!("{}", dir.display()), e))?;
    if ancestors.contains(&real) {
        c.error(
            prefix.trim_end_matches('/'),
            "a symlink to a folder that contains it (a loop): remove it",
        );
        return Ok(());
    }
    ancestors.push(real);
    let r = walk_entries(root, dir, prefix, c, ancestors);
    ancestors.pop();
    r
}

fn walk_entries(
    root: &Path,
    dir: &Path,
    prefix: &str,
    c: &mut Checked,
    ancestors: &mut Vec<PathBuf>,
) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| Error::io(format!("{}", dir.display()), e))?;
    let mut entries: Vec<std::fs::DirEntry> = rd.filter_map(std::result::Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let os = e.file_name();
        let Some(name) = os.to_str() else {
            c.error(
                format!("{prefix}{}", os.to_string_lossy()),
                "the name is not UTF-8: rename it",
            );
            continue;
        };
        let rel = format!("{prefix}{name}");
        if name.starts_with('.') {
            c.skipped += 1;
            continue;
        }
        if let Some(why) = name_problem(name) {
            c.error(&rel, why);
            continue;
        }
        if windows_problem(name) {
            c.warn(
                &rel,
                "the name cannot be unpacked on Windows (<>:\"|?* or a trailing dot or space)",
            );
        }
        if prefix.is_empty() && name == "_tvpipe" {
            c.error(
                &rel,
                "/_tvpipe/ is tvpipe's own: a game's _tvpipe folder is never served; rename it",
            );
            continue;
        }
        let path = e.path();
        let lmeta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(err) => return Err(Error::io(format!("{}", path.display()), err)),
        };
        let (meta, real) = if lmeta.file_type().is_symlink() {
            let Ok(real) = std::fs::canonicalize(&path) else {
                c.error(&rel, "a broken symlink: remove it or copy the file in");
                continue;
            };
            if !real.starts_with(root) {
                c.error(
                    &rel,
                    format!(
                        "a symlink to {}, outside the game's root: never served and not packed; copy it in",
                        real.display()
                    ),
                );
                continue;
            }
            match std::fs::metadata(&real) {
                Ok(m) => (m, real),
                Err(err) => return Err(Error::io(format!("{}", real.display()), err)),
            }
        } else {
            (lmeta, path)
        };
        if meta.is_dir() {
            if prefix.is_empty() && name == "node_modules" {
                c.warn(
                    &rel,
                    "node_modules is served and packed: point \"root\" at the build output",
                );
            }
            walk(root, &real, &format!("{rel}/"), c, ancestors)?;
        } else if meta.is_file() {
            if c.files.len() >= MAX_FILES {
                c.error(
                    "",
                    format!("more than {MAX_FILES} files: a bundle holds at most {MAX_FILES}"),
                );
                return Ok(());
            }
            let bytes = std::fs::read(&real)
                .map_err(|err| Error::io(format!("read {}", real.display()), err))?;
            c.files.insert(rel, bytes);
        } else {
            c.warn(&rel, "not a file or a folder: skipped");
        }
    }
    Ok(())
}

/// A zip entry name that could write outside an unzip tool's folder.
fn unsafe_name(n: &str) -> bool {
    n.starts_with('/')
        || n.split('/').any(|s| s == "..")
        || n.contains('\\')
        || n.as_bytes().get(1) == Some(&b':')
        || n.contains('\0')
}

fn gather_zip(data: &[u8], c: &mut Checked) {
    let entries = match zip::read(data, zip::Limits::default()) {
        Ok(e) => e,
        Err(e) => {
            let msg = e.message().to_owned();
            c.error(
                "",
                format!("{msg} (tvpipe game pack DIR writes zips tvpipe reads)"),
            );
            return;
        }
    };
    // Names: unsafe ones, duplicates, case.
    let mut seen = BTreeSet::new();
    let mut ok: Vec<&zip::Entry> = Vec::new();
    for e in &entries {
        let n = &e.name;
        let segs: Vec<&str> = n.split('/').collect();
        if unsafe_name(n) {
            c.error(
                n.as_str(),
                "an unsafe name (absolute, .., a backslash or a drive): unzip tools may write outside their folder; tvpipe never serves it",
            );
            continue;
        }
        if !seen.insert(n.clone()) {
            c.error(n.as_str(), "twice in the archive");
            continue;
        }
        if segs.first() == Some(&"__MACOSX") {
            c.skipped += 1;
            continue;
        }
        if segs.iter().any(|s| s.starts_with('.')) {
            c.skipped += 1;
            continue;
        }
        if n.chars().any(char::is_control) {
            c.error(n.as_str(), "has control characters: never served");
            continue;
        }
        if segs.iter().any(|s| windows_problem(s)) {
            c.warn(
                n.as_str(),
                "the name cannot be unpacked on Windows (<>:\"|?* or a trailing dot or space)",
            );
        }
        ok.push(e);
    }
    let macosx = entries.iter().any(|e| e.name.starts_with("__MACOSX/"));
    // tvpipe.json at the top, or in the one top-level folder.
    let prefix = if ok.iter().any(|e| e.name == MANIFEST) {
        String::new()
    } else {
        // As playshare sees it: what archivers add beside the folder
        // (Finder's __MACOSX/, hidden names) does not count.
        let tops: BTreeSet<&str> = entries
            .iter()
            .filter(|e| !unsafe_name(&e.name))
            .map(|e| e.name.split('/').next().unwrap_or(""))
            .filter(|t| !t.starts_with('.') && *t != "__MACOSX")
            .collect();
        match tops.iter().next() {
            Some(t)
                if tops.len() == 1 && ok.iter().any(|e| e.name == format!("{t}/{MANIFEST}")) =>
            {
                format!("{t}/")
            }
            _ => {
                let deeper = ok
                    .iter()
                    .find(|e| e.name.ends_with(&format!("/{MANIFEST}")))
                    .map(|e| e.name.clone());
                let tvpipe_top = ok.iter().find_map(|e| {
                    let (t, rest) = e.name.split_once('/')?;
                    (rest == MANIFEST).then_some(t)
                });
                let hint = match deeper {
                    _ if tvpipe_top.is_some() => format!(
                        " ({}/ has it, but the archive has more at its top: {}; build the zip with tvpipe game pack DIR)",
                        tvpipe_top.unwrap_or(""),
                        tops.iter().filter(|t| Some(**t) != tvpipe_top).copied().collect::<Vec<_>>().join(", ")
                    ),
                    Some(d) => format!(
                        " ({d} is deeper: zip the game's folder itself, or use tvpipe game pack DIR)"
                    ),
                    None => String::new(),
                };
                c.error(
                    "",
                    format!("no {MANIFEST} at the top of the archive or in its one top-level folder{hint}"),
                );
                return;
            }
        }
    };
    if macosx {
        c.warn(
            "__MACOSX/",
            "Finder's resource forks: harmless, but tvpipe game pack DIR makes a clean zip",
        );
    }
    let mtext = ok
        .iter()
        .find(|e| e.name == format!("{prefix}{MANIFEST}"))
        .map(|e| e.data.clone())
        .unwrap_or_default();
    parse_manifest(&mtext, c);
    let Some(m) = c.manifest.clone() else {
        return;
    };
    let base = if m.root.is_empty() {
        prefix.clone()
    } else {
        format!("{prefix}{}/", m.root)
    };
    let mut outside = 0;
    for e in ok {
        let Some(rel) = e.name.strip_prefix(&base) else {
            if e.name != format!("{prefix}{MANIFEST}") {
                outside += 1;
            }
            continue;
        };
        if rel == "_tvpipe" || rel.starts_with("_tvpipe/") {
            c.error(
                e.name.as_str(),
                "/_tvpipe/ is tvpipe's own: never served; rename it",
            );
            continue;
        }
        match clean_path(rel) {
            Some(rel) => {
                c.files.insert(rel, e.data.clone());
            }
            None => c.error(e.name.as_str(), "not a clean path: never served"),
        }
    }
    if outside > 0 {
        c.warn(
            "",
            format!("{outside} files outside \"root\" {:?}: never served (left out of tvpipe game pack's zips)", m.root),
        );
    }
    if c.files.is_empty() {
        c.error(
            "",
            format!("no files below \"root\" {:?} in the archive", m.root),
        );
    }
}

/// The files as a whole: case collisions, sizes.
fn check_files(c: &mut Checked) {
    let mut lower: BTreeMap<String, &str> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut total = 0u64;
    for (p, data) in &c.files {
        // Folders collide too: a/X.png and A/y.png.
        if let Some(other) = lower.insert(p.to_lowercase(), p) {
            problems.push((
                Level::Error,
                p.clone(),
                format!(
                    "differs from {other} only in case: one of them is lost on macOS and Windows"
                ),
            ));
        }
        let size = data.len() as u64;
        total += size;
        if size > BIG_FILE {
            problems.push((
                Level::Warning,
                p.clone(),
                format!(
                    "{}: large files make phones wait; shrink it if phones load it",
                    human(size)
                ),
            ));
        }
    }
    let mut dirs: BTreeMap<String, String> = BTreeMap::new();
    for p in c.files.keys() {
        let mut acc = String::new();
        for seg in p
            .split('/')
            .collect::<Vec<_>>()
            .split_last()
            .map_or(&[][..], |s| s.1)
        {
            acc.push_str(seg);
            if let Some(other) = dirs.insert(acc.to_lowercase(), acc.clone()) {
                if other != acc {
                    problems.push((
                        Level::Error,
                        acc.clone(),
                        format!("the folder differs from {other} only in case: they merge on macOS and Windows"),
                    ));
                }
            }
            acc.push('/');
        }
    }
    for (l, at, m) in problems {
        c.add(l, at, m);
    }
    if total > MAX_TOTAL {
        c.error(
            "",
            format!(
                "{} of files: a bundle holds at most {}",
                human(total),
                human(MAX_TOTAL)
            ),
        );
    }
    if c.files.len() + 1 > MAX_FILES {
        c.error(
            "",
            format!(
                "{} files: a bundle holds at most {MAX_FILES}",
                c.files.len()
            ),
        );
    }
}

/// Where a URL leads.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// Not a file: `#x`, `data:`, `mailto:`, empty, a template.
    Skip,
    Remote(String),
    /// A path below `/_tvpipe/` (without that prefix).
    Tvpipe(String),
    /// A path below `root`.
    File(String),
    /// A bare module specifier (`@tvpipe/party`).
    Bare(String),
    Bad(String),
}

fn resolve(from: &str, url: &str, module: bool) -> Target {
    let u = url.trim();
    if u.is_empty() || u.starts_with('#') || u.contains("${") || u.contains("{{") {
        return Target::Skip;
    }
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || u.starts_with("//") {
        return Target::Remote(u.to_owned());
    }
    // Another scheme (data:, blob:, mailto:, javascript:, …).
    if let Some(colon) = u.find(':') {
        if u[..colon]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
            && u[..colon]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic())
        {
            return Target::Skip;
        }
    }
    if module && !(u.starts_with('/') || u.starts_with("./") || u.starts_with("../")) {
        return Target::Bare(u.to_owned());
    }
    let path = u.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() {
        // `?x=1`: the page itself.
        return Target::File(from.to_owned());
    }
    let Some(path) = percent_decode(path) else {
        return Target::Bad(format!("{u:?} has a bad %-escape"));
    };
    let dir_like = path.ends_with('/');
    let mut segs: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        let mut d: Vec<&str> = from.split('/').collect();
        d.pop();
        d
    };
    for s in path.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                if segs.pop().is_none() {
                    return Target::Bad(format!("{u:?} leads above the game's root"));
                }
            }
            s => segs.push(s),
        }
    }
    let mut rel = segs.join("/");
    if let Some(rest) = rel.strip_prefix("_tvpipe/") {
        return Target::Tvpipe(rest.to_owned());
    }
    if rel == "_tvpipe" {
        return Target::Tvpipe(String::new());
    }
    if dir_like || rel.is_empty() {
        rel = if rel.is_empty() {
            "index.html".to_owned()
        } else {
            format!("{rel}/index.html")
        };
    }
    Target::File(rel)
}

/// The import map of a page: exact and prefix (`…/`) entries.
#[derive(Default, Clone)]
struct ImportMap(Vec<(String, String)>);

impl ImportMap {
    fn lookup(&self, spec: &str) -> Option<String> {
        if let Some((_, v)) = self.0.iter().find(|(k, _)| k == spec) {
            return Some(v.clone());
        }
        self.0
            .iter()
            .filter(|(k, _)| k.ends_with('/') && spec.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(k, v)| format!("{v}{}", &spec[k.len()..]))
    }
}

/// What one entry page reaches.
#[derive(Default)]
struct Reach {
    /// `lib/…` modules of tvpipe's library.
    lib: BTreeSet<String>,
    /// Streams a canvas to the TVs (a surface): the TV page then need not
    /// be a client.
    surface: bool,
    seen: HashSet<String>,
}

/// Whether a script streams a canvas (`tvpipe.surface(…)`,
/// `openSurface(…)`, `SurfaceEncoder`).
fn uses_surface(text: &str) -> bool {
    text.contains(".surface(") || text.contains("openSurface(") || text.contains("SurfaceEncoder")
}

fn what(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Script { .. } => "script",
        RefKind::ModulePreload => "module preload",
        RefKind::Stylesheet => "style sheet",
        RefKind::Frame => "frame",
        RefKind::Link => "link",
        RefKind::Asset => "file",
        RefKind::Import => "import",
        RefKind::DynamicImport => "import()",
    }
}

fn library_hint(spec: &str) -> Option<&'static str> {
    match spec {
        "@tvpipe/party" => Some("/_tvpipe/lib/index.js"),
        "@tvpipe/party/client" => Some("/_tvpipe/lib/client.js"),
        "@tvpipe/party/testing" => Some("/_tvpipe/lib/testing.js"),
        _ => None,
    }
}

struct Walker<'a> {
    files: &'a BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    lower: BTreeMap<String, String>,
    problems: Vec<(Level, String, String)>,
}

impl Walker<'_> {
    fn at(file: &str, line: u32) -> String {
        if line == 0 {
            file.to_owned()
        } else {
            format!("{file}:{line}")
        }
    }

    fn page(&mut self, page: &str) -> Reach {
        let mut reach = Reach::default();
        self.html(page, &mut reach);
        reach
    }

    fn html(&mut self, file: &str, reach: &mut Reach) {
        if !reach.seen.insert(file.to_owned()) {
            return;
        }
        let Some(bytes) = self.files.get(file) else {
            return;
        };
        let text = String::from_utf8_lossy(bytes).into_owned();
        let h = scan::html(&text);
        if let Some((_, line)) = &h.base {
            self.problems.push((
                Level::Warning,
                Self::at(file, *line),
                "<base href> changes how every link resolves: the check follows them without it"
                    .into(),
            ));
        }
        let mut map = ImportMap::default();
        for m in &h.import_maps {
            match Value::parse(&m.text) {
                Ok(v) => {
                    if let Some(o) = v.get("imports").and_then(Value::as_object) {
                        for (k, val) in o.iter() {
                            if let Some(s) = val.as_str() {
                                map.0.push((k.to_owned(), s.to_owned()));
                            }
                        }
                    }
                }
                Err(e) => self.problems.push((
                    Level::Error,
                    Self::at(file, m.line),
                    format!("the import map is not JSON: {}", e.message()),
                )),
            }
        }
        for r in &h.refs {
            self.reference(file, r, &map, reach);
        }
        for s in &h.scripts {
            reach.surface |= uses_surface(&s.text);
            for mut r in scan::js(&s.text) {
                r.line += s.line - 1;
                self.reference(file, &r, &map, reach);
            }
        }
        for s in &h.styles {
            for mut r in scan::css(&s.text) {
                r.line += s.line - 1;
                self.reference(file, &r, &map, reach);
            }
        }
    }

    fn reference(&mut self, file: &str, r: &Ref, map: &ImportMap, reach: &mut Reach) {
        let module = matches!(r.kind, RefKind::Import | RefKind::DynamicImport);
        let at = Self::at(file, r.line);
        let link = r.kind == RefKind::Link;
        let miss = if link { Level::Warning } else { Level::Error };
        let mut target = resolve(file, &r.url, module);
        if let Target::Bare(spec) = &target {
            match map.lookup(spec) {
                Some(mapped) => target = resolve(file, &mapped, false),
                None => {
                    let hint = match library_hint(spec) {
                        Some(p) => format!("import {p:?} (tvpipe serves @tvpipe/party there)"),
                        None => "use a path (\"./x.js\", \"/x.js\") or an import map; a bundler or your build step rewrites these".into(),
                    };
                    self.problems.push((
                        Level::Error,
                        at,
                        format!("bare import {spec:?}: browsers cannot resolve it; {hint}"),
                    ));
                    return;
                }
            }
        }
        match target {
            Target::Skip | Target::Bare(_) => {}
            Target::Bad(why) => self.problems.push((miss, at, why)),
            Target::Remote(url) => {
                if !link {
                    self.problems.push((
                        Level::Warning,
                        at,
                        format!(
                            "{} from the internet ({url}): phones on --lan and TVs without internet do not get it; put a copy in the game",
                            what(r.kind)
                        ),
                    ));
                }
            }
            Target::Tvpipe(rest) => {
                let ok = if rest.starts_with("lib/") {
                    web::FILES.iter().any(|f| f.path == rest)
                } else {
                    rest == "launcher/"
                        || rest.starts_with("launcher/")
                        || rest == "ws"
                        || rest == "api"
                        || rest == "ui.css"
                        || rest
                            .strip_prefix("fonts/")
                            .is_some_and(|f| fonts::font(f).is_some())
                };
                if !ok {
                    let libs: Vec<&str> = web::FILES
                        .iter()
                        .filter_map(|f| f.path.strip_prefix("lib/"))
                        .collect();
                    self.problems.push((
                        Level::Error,
                        at,
                        format!(
                            "/_tvpipe/{rest} does not exist: tvpipe serves its library as /_tvpipe/lib/ ({}) and its look as /_tvpipe/ui.css",
                            libs.join(", ")
                        ),
                    ));
                } else if rest.starts_with("lib/") {
                    reach.lib.insert(rest);
                }
            }
            Target::File(rel) => {
                let rel = if self.files.contains_key(&rel) {
                    rel
                } else if self.dirs.contains(&rel) {
                    let index = format!("{rel}/index.html");
                    if self.files.contains_key(&index) {
                        index
                    } else {
                        self.problems.push((
                            miss,
                            at,
                            format!(
                                "{} {:?}: the folder {rel}/ has no index.html",
                                what(r.kind),
                                r.url
                            ),
                        ));
                        return;
                    }
                } else {
                    self.problems
                        .push((miss, at, self.missing(&r.url, &rel, r.kind)));
                    return;
                };
                let script = matches!(
                    r.kind,
                    RefKind::Script { .. }
                        | RefKind::Import
                        | RefKind::DynamicImport
                        | RefKind::ModulePreload
                );
                if script && [".ts", ".tsx", ".jsx"].iter().any(|e| rel.ends_with(e)) {
                    self.problems.push((
                        Level::Error,
                        at,
                        format!("{rel}: browsers do not run TypeScript or JSX; load the built .js"),
                    ));
                    return;
                }
                match r.kind {
                    RefKind::Script { module } => {
                        if !module && self.uses_import(&rel) {
                            self.problems.push((
                                Level::Error,
                                at.clone(),
                                format!("{rel} uses import but is loaded as a classic script: add type=\"module\""),
                            ));
                        }
                        self.js(&rel, map, reach);
                    }
                    RefKind::Import | RefKind::DynamicImport | RefKind::ModulePreload => {
                        self.js(&rel, map, reach)
                    }
                    RefKind::Stylesheet => self.css(&rel, map, reach),
                    RefKind::Frame | RefKind::Link => {
                        if scan::kind_of(&rel) == Kind::Html && r.kind == RefKind::Frame {
                            self.html(&rel, reach);
                        }
                    }
                    RefKind::Asset => {}
                }
            }
        }
    }

    fn missing(&self, url: &str, rel: &str, kind: RefKind) -> String {
        let mut msg = format!("{} {url:?}: {rel} is not in the game", what(kind));
        if rel.split('/').any(|s| s.starts_with('.')) {
            msg = format!(
                "{} {url:?}: {rel} is hidden (a name starting with .): never served",
                what(kind)
            );
        } else if let Some(other) = self.lower.get(&rel.to_lowercase()) {
            msg.push_str(&format!(
                " ({other} differs in case: that works from a folder on macOS and Windows, not from a zip or on Linux)"
            ));
        } else if matches!(kind, RefKind::Import | RefKind::DynamicImport)
            && self.files.contains_key(&format!("{rel}.js"))
        {
            msg.push_str(&format!(
                " (did you mean {rel}.js? browsers add no extensions)"
            ));
        } else if self.files.contains_key(&rel.replace(".js", ".ts")) && rel.ends_with(".js") {
            msg.push_str(" (there is a .ts of that name: build the game?)");
        }
        msg
    }

    fn uses_import(&self, rel: &str) -> bool {
        self.files.get(rel).is_some_and(|b| {
            scan::js(&String::from_utf8_lossy(b))
                .iter()
                .any(|r| r.kind == RefKind::Import)
        })
    }

    fn js(&mut self, file: &str, map: &ImportMap, reach: &mut Reach) {
        if !reach.seen.insert(file.to_owned()) {
            return;
        }
        let Some(bytes) = self.files.get(file) else {
            return;
        };
        let text = String::from_utf8_lossy(bytes).into_owned();
        reach.surface |= uses_surface(&text);
        if text.contains("class PartyClient") || text.contains("class HostedParty") {
            self.problems.push((
                Level::Warning,
                file.to_owned(),
                "has a copy of @tvpipe/party: it must speak the API of the tvpipe that serves the game; import tvpipe's own from /_tvpipe/lib/ instead".into(),
            ));
            // A copy counts as the library for the pages' checks.
            reach.lib.insert("lib/index.js".into());
            reach.lib.insert("lib/client.js".into());
        }
        for r in scan::js(&text) {
            self.reference(file, &r, map, reach);
        }
    }

    fn css(&mut self, file: &str, map: &ImportMap, reach: &mut Reach) {
        if !reach.seen.insert(file.to_owned()) {
            return;
        }
        let Some(bytes) = self.files.get(file) else {
            return;
        };
        let text = String::from_utf8_lossy(bytes).into_owned();
        for r in scan::css(&text) {
            self.reference(file, &r, map, reach);
        }
    }
}

/// The entry pages and what they load.
fn check_pages(c: &mut Checked) {
    let Some(m) = c.manifest.clone() else { return };
    let mut pages = vec![
        ("host", m.pages.host.clone()),
        ("tv", m.pages.tv.clone()),
        ("phone", m.pages.phone.clone()),
    ];
    if let Some(l) = &m.pages.launcher {
        pages.push(("launcher", l.clone()));
    }
    let mut dirs = BTreeSet::new();
    for p in c.files.keys() {
        let mut d = p.as_str();
        while let Some((parent, _)) = d.rsplit_once('/') {
            dirs.insert(parent.to_owned());
            d = parent;
        }
    }
    let lower = c
        .files
        .keys()
        .map(|k| (k.to_lowercase(), k.clone()))
        .collect();
    let mut w = Walker {
        files: &c.files,
        dirs,
        lower,
        problems: Vec::new(),
    };
    let root = if m.root.is_empty() {
        String::new()
    } else {
        format!("{}/", m.root)
    };
    let mut found = Vec::new();
    for (key, page) in &pages {
        if !w.files.contains_key(page) {
            let mut msg = format!("\"pages.{key}\": {root}{page} is not in the game");
            if let Some(other) = w.lower.get(&page.to_lowercase()) {
                msg.push_str(&format!(" ({root}{other} differs in case)"));
            } else if !root.is_empty() {
                msg.push_str(" (is the game built?)");
            }
            w.problems.push((Level::Error, MANIFEST.into(), msg));
            continue;
        }
        if scan::kind_of(page) != Kind::Html {
            w.problems.push((
                Level::Warning,
                MANIFEST.into(),
                format!("\"pages.{key}\": {page} is not an .html page"),
            ));
        }
        let reach = w.page(page);
        found.push((*key, page.clone(), reach));
    }
    let surfaces = found
        .iter()
        .any(|(_, _, r)| r.surface || r.lib.contains("lib/surface.js"));
    for (key, page, reach) in &found {
        let has = |m: &str| reach.lib.contains(m);
        match *key {
            "host" if !(has("lib/index.js") || has("lib/tvpipe.js")) => w.problems.push((
                Level::Warning,
                page.clone(),
                "the host page does not load @tvpipe/party (import { Tvpipe } from \"/_tvpipe/lib/index.js\"): without it no relay is attached and phones cannot join".into(),
            )),
            // With surfaces the TV picture comes from the host's canvases.
            "tv" if surfaces => {}
            "tv" | "phone" if !(has("lib/client.js") || has("lib/index.js")) => w.problems.push((
                Level::Warning,
                page.clone(),
                format!(
                    "the {key} page does not load @tvpipe/party's client (import {{ PartyClient }} from \"/_tvpipe/lib/client.js\")"
                ),
            )),
            _ => {}
        }
    }
    for (l, at, msg) in std::mem::take(&mut w.problems) {
        c.add(l, at, msg);
    }
}

/// The zip of a checked folder: `NAME/tvpipe.json` (without `root`, the
/// files are at its side) and `NAME/…` for every served file. Refused
/// while there are errors.
pub fn pack(c: &Checked) -> Result<Vec<u8>> {
    if c.errors() > 0 {
        return Err(Error::usage(format!(
            "{}: {} errors: fix them first",
            c.origin.display(),
            c.errors()
        )));
    }
    let (Some(m), Some(json)) = (&c.manifest, &c.manifest_json) else {
        return Err(Error::usage("no manifest"));
    };
    let prefix = format!("{}/", m.name);
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(c.files.len() + 1);
    let manifest = if m.root.is_empty() {
        c.files.get(MANIFEST).cloned()
    } else {
        None
    };
    let manifest = manifest.unwrap_or_else(|| {
        let mut v = json.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("root");
        }
        (v.to_json_pretty() + "\n").into_bytes()
    });
    entries.push((format!("{prefix}{MANIFEST}"), manifest));
    for (p, data) in &c.files {
        if p == MANIFEST {
            continue;
        }
        entries.push((format!("{prefix}{p}"), data.clone()));
    }
    let bytes = crate::util::zip_write::write(&entries)?;
    // Read back the way playshare reads it.
    let b = Bundle::from_zip(&bytes, &c.origin)
        .map_err(|e| Error::closed(format!("the written zip does not read back: {e}")))?;
    let mut back = Manifest::parse(&String::from_utf8_lossy(&entries[0].1))?;
    back.root = m.root.clone();
    if back != *m || !b.manifest.root.is_empty() {
        return Err(Error::closed("the written zip's manifest differs"));
    }
    for (p, data) in &c.files {
        match b.get(p) {
            super::bundle::Lookup::File(f) if f.bytes == *data => {}
            _ => {
                return Err(Error::closed(format!(
                    "the written zip's {p} does not read back"
                )))
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
