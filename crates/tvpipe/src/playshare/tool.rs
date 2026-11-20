//! `tvpipe game pack`, `game check`, `game new`, `game fetch` and `game
//! list`: pack a game into a zip, check it, run it once without TVs, start
//! a new one from the template, download one from the internet, list the
//! downloaded ones (docs/games.md).

use std::path::{Path, PathBuf};

use super::cache::Cache;
use super::check::{self, Checked};
use super::remote::{FetchOptions, Fetcher};
use super::smoke::{self, SmokeOptions};
use super::source::Source;
use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::events::{Event, Events, Progress, Step};
use crate::util::stop::Stop;

/// Which of the game commands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolAction {
    /// `game pack DIR`: check the folder and write the zip.
    #[default]
    Pack,
    /// `game check PATH`: a folder or a zip; write nothing.
    Check,
    /// `game new DIR`: the starter game.
    New,
    /// `game fetch SOURCE`: download into the cache (and `-o FILE`).
    Fetch,
    /// `game list`: the games in the cache.
    List,
}

impl ToolAction {
    /// The command line's words (`game pack`).
    pub fn command(self) -> &'static str {
        match self {
            ToolAction::Pack => "game pack",
            ToolAction::Check => "game check",
            ToolAction::New => "game new",
            ToolAction::Fetch => "game fetch",
            ToolAction::List => "game list",
        }
    }
}

/// The options of `tvpipe game pack`, `game check` and `game new`.
#[derive(Debug, Clone, Default)]
pub struct ToolOptions {
    pub action: ToolAction,
    /// The game: a folder (packed by `Pack`) or a `.zip` (`Check`); the
    /// new folder for `New`.
    pub path: PathBuf,
    /// `-o FILE`: the zip (default `NAME.zip` here).
    pub out: Option<PathBuf>,
    /// `--smoke`: also run it once in a headless browser.
    pub smoke: bool,
    /// `--strict`: warnings fail too.
    pub strict: bool,
    /// `--browser`: for `--smoke`.
    pub browser: Option<PathBuf>,
    /// `--refresh`, `--offline` (a game from the internet).
    pub fetch: FetchOptions,
}

/// Runs a game command; the report goes to stdout. An error when the
/// game has errors (or warnings with `strict`).
pub fn run(o: &ToolOptions, stop: Stop) -> Result<()> {
    let cmd = o.action.command();
    if o.action == ToolAction::New {
        let files = init(&o.path)?;
        let d = o.path.display();
        println!("made a starter game in {d} ({} files)", files.len());
        println!();
        println!("  tvpipe game play {d}       play it (edit a file, reload the page)");
        println!("  tvpipe game check {d}      check it");
        println!("  tvpipe game pack {d}       pack it into a zip for others");
        return Ok(());
    }
    if o.action == ToolAction::List {
        return list();
    }
    let game = o.path.to_string_lossy().into_owned();
    let src = Source::parse(&game)?;
    let fetched = match &src {
        Source::Local(_) if o.action == ToolAction::Fetch => {
            return Err(Error::usage(format!(
                "game fetch: {game} is on this computer already (a game from the internet is https://…, gh:… or a name)"
            )))
        }
        Source::Local(_) => None,
        _ if o.action == ToolAction::Pack => {
            return Err(Error::usage(format!(
                "game pack: {game} is not a folder on this computer"
            )))
        }
        _ => {
            let fetcher = Fetcher::from_env(o.fetch, download_lines(), Some(stop.clone()))?;
            Some(fetcher.fetch(&src)?)
        }
    };
    if let Some(f) = &fetched {
        let m = &f.bundle.manifest;
        println!(
            "{} (\"{}\"){} from {}",
            m.name,
            m.title,
            m.tvpipe
                .as_deref()
                .map(|v| format!(", made for tvpipe {v}"))
                .unwrap_or_default(),
            f.shown
        );
        if let Some(t) = &f.tag {
            println!("  release {t}");
        }
        println!("  {}, sha256 {}", check::human(f.size), f.sha256);
        println!("  cached at {}", f.path.display());
        println!("  pinned: tvpipe game play '{}'", f.pin_hint);
        if o.action == ToolAction::Fetch {
            if let Some(out) = &o.out {
                let bytes = std::fs::read(&f.path)
                    .map_err(|e| Error::io(format!("read {}", f.path.display()), e))?;
                write_atomic(out, &bytes)?;
                println!("wrote {}", out.display());
            }
            return Ok(());
        }
    }
    let path = fetched.as_ref().map_or(o.path.clone(), |f| f.path.clone());
    let mut c = check::check(&path)?;
    if let Some(f) = &fetched {
        c.origin = PathBuf::from(&f.key);
    }
    let out = if o.action == ToolAction::Pack {
        if c.is_zip {
            return Err(Error::usage(format!(
                "game pack: {} is a zip already (tvpipe game check checks it)",
                o.path.display()
            )));
        }
        let name = c.manifest.as_ref().map_or("game", |m| m.name.as_str());
        let out = o
            .out
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("{name}.zip")));
        if c.exclude(&out) {
            c.note(format!(
                "{} is inside the game's root: left out of the zip",
                out.display()
            ));
        }
        Some(out)
    } else {
        None
    };
    print!("{}", c.render());
    fail_on(&c, cmd, o.strict)?;
    let target = match &out {
        Some(out) => {
            let bytes = check::pack(&c)?;
            write_atomic(out, &bytes)?;
            let (_, size) = c.size();
            println!(
                "wrote {} ({}, {} unpacked)",
                out.display(),
                check::human(bytes.len() as u64),
                check::human(size)
            );
            println!(
                "sha256 {}",
                crate::util::hex::encode(&crate::crypto::ring::sha256(&bytes))
            );
            out.clone()
        }
        None => path.clone(),
    };
    if o.smoke {
        let r = smoke::smoke(
            &target,
            &SmokeOptions {
                browser: o.browser.clone(),
                stop: Some(stop),
                ..SmokeOptions::default()
            },
        )?;
        print!("{}", r.render());
        let warnings = r.problems.len() - r.errors();
        if r.errors() > 0 || (o.strict && warnings > 0) {
            return Err(Error::usage(format!(
                "{cmd}: the smoke run found {} errors and {} warnings",
                r.errors(),
                warnings
            )));
        }
        println!("smoke: ok");
    }
    Ok(())
}

/// Progress of a download as lines on stderr (the report is stdout's).
fn download_lines() -> Events {
    Events::to(|e| match e {
        Event::Progress(Progress::Begin(Step::Download(what))) => {
            eprintln!("downloading {what}…")
        }
        Event::Progress(Progress::Failed(Step::Download(what), err)) => {
            eprintln!("downloading {what} failed: {err}")
        }
        _ => {}
    })
}

/// `game list`: the games in the cache, the last played first.
fn list() -> Result<()> {
    let cache = Cache::open()?;
    let idx = cache.index();
    let mut rows: Vec<_> = idx
        .sources
        .iter()
        .filter(|(_, r)| cache.blob(&r.sha256).is_some())
        .collect();
    if rows.is_empty() {
        println!("no games downloaded yet ({})", cache.dir().display());
        return Ok(());
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.1.used));
    for (source, r) in rows {
        println!(
            "{:<20} {}{}",
            r.name,
            source,
            r.tag
                .as_deref()
                .filter(|t| !source.contains(&format!("@{t}")))
                .map(|t| format!(" ({t})"))
                .unwrap_or_default()
        );
        println!(
            "{:<20} {}, sha256 {}…{}{}",
            "",
            check::human(r.size),
            &r.sha256[..16],
            r.series
                .as_deref()
                .map(|s| format!(", for tvpipe {s}"))
                .unwrap_or_default(),
            if r.trusted.as_deref() == Some(r.sha256.as_str()) {
                ""
            } else {
                ", not played yet"
            }
        );
    }
    println!("cache: {}", cache.dir().display());
    Ok(())
}

fn fail_on(c: &Checked, cmd: &str, strict: bool) -> Result<()> {
    if c.errors() > 0 {
        return Err(Error::usage(format!(
            "{cmd}: {} has {} error{}{}",
            c.origin.display(),
            c.errors(),
            if c.errors() == 1 { "" } else { "s" },
            if c.is_zip { "" } else { "; nothing written" }
        )));
    }
    if strict && c.warnings() > 0 {
        return Err(Error::usage(format!(
            "{cmd}: {} has {} warnings (--strict)",
            c.origin.display(),
            c.warnings()
        )));
    }
    Ok(())
}

/// Writes through a temporary file next to `path`, then renames.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("zip.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(format!("write {}", tmp.display()), e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(format!("write {}", path.display()), e)
    })
}

/// The template (`crates/tvpipe/src/playshare/template/`).
const TEMPLATE: &[(&str, &str)] = &[
    ("README.md", include_str!("template/README.md")),
    ("tvpipe.json", include_str!("template/tvpipe.json")),
    ("style.css", include_str!("template/style.css")),
    ("host/index.html", include_str!("template/host/index.html")),
    ("host/host.css", include_str!("template/host/host.css")),
    ("host/host.js", include_str!("template/host/host.js")),
    ("tv/index.html", include_str!("template/tv/index.html")),
    ("tv/tv.css", include_str!("template/tv/tv.css")),
    ("tv/tv.js", include_str!("template/tv/tv.js")),
    (
        "phone/index.html",
        include_str!("template/phone/index.html"),
    ),
    ("phone/phone.css", include_str!("template/phone/phone.css")),
    ("phone/phone.js", include_str!("template/phone/phone.js")),
];

/// The manifest name for a folder name: `My Game!` → `my-game`.
pub fn name_for(dir: &str) -> String {
    let mut s = String::new();
    for ch in dir.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            s.push(ch);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_matches('-').chars().take(40).collect();
    let s = s.trim_end_matches('-').to_owned();
    if s.is_empty() {
        "my-game".into()
    } else {
        s
    }
}

/// Writes the starter game into `dir` (new, or empty but for hidden
/// files); returns the files written.
pub fn init(dir: &Path) -> Result<Vec<PathBuf>> {
    if let Ok(rd) = std::fs::read_dir(dir) {
        let busy = rd
            .filter_map(std::result::Result::ok)
            .any(|e| !e.file_name().to_string_lossy().starts_with('.'));
        if busy {
            return Err(Error::usage(format!(
                "game new: {} is not empty",
                dir.display()
            )));
        }
    }
    let base = dir
        .canonicalize()
        .ok()
        .or_else(|| Some(dir.to_owned()))
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let name = name_for(&base);
    let mut title: String = name.replace('-', " ");
    if let Some(first) = title.get(..1) {
        title = first.to_uppercase() + &title[1..];
    }
    let json_title = Value::from(title.as_str()).to_json();
    let json_title = &json_title[1..json_title.len() - 1];
    let mut written = Vec::new();
    for (rel, text) in TEMPLATE {
        let title = match *rel {
            "tvpipe.json" => json_title.to_owned(),
            r if r.ends_with(".html") => super::web::html_escape(&title),
            _ => title.clone(),
        };
        let text = text
            .replace("{{name}}", &name)
            .replace("{{title}}", &title)
            .replace("{{tvpipe}}", &check::short_version(check::VERSION));
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io(format!("create {}", parent.display()), e))?;
        }
        std::fs::write(&path, text)
            .map_err(|e| Error::io(format!("write {}", path.display()), e))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp/bundle-tool")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names() {
        assert_eq!(name_for("My Game!"), "my-game");
        assert_eq!(name_for("--"), "my-game");
        assert_eq!(name_for("Ärger 2"), "rger-2");
        assert_eq!(name_for(&"x".repeat(60)).len(), 40);
    }

    /// The starter game checks clean, packs, and its zip checks clean.
    #[test]
    fn the_template_is_clean() {
        let dir = scratch("Quiz Night").join("Quiz Night");
        let files = init(&dir).unwrap();
        assert_eq!(files.len(), TEMPLATE.len());
        assert!(init(&dir).unwrap_err().to_string().contains("not empty"));
        let c = check::check(&dir).unwrap();
        assert!(c.problems.is_empty(), "{}", c.render());
        let m = c.manifest.as_ref().unwrap();
        assert_eq!(
            (m.name.as_str(), m.title.as_str()),
            ("quiz-night", "Quiz night")
        );
        let html = std::fs::read_to_string(dir.join("host/index.html")).unwrap();
        assert!(html.contains("<title>Quiz night</title>"));
        // Packing into the game's own folder leaves the zip out.
        let o = ToolOptions {
            path: dir.clone(),
            out: Some(dir.join("quiz-night.zip")),
            ..ToolOptions::default()
        };
        run(&o, Stop::new()).unwrap();
        run(&o, Stop::new()).unwrap();
        let z = check::check(&dir.join("quiz-night.zip")).unwrap();
        assert!(z.problems.is_empty(), "{}", z.render());
        assert!(!z.paths().any(|p| p.ends_with(".zip")));
        // A zip is checked, not packed again.
        let mut again = ToolOptions {
            path: dir.join("quiz-night.zip"),
            out: Some(dir.join("x.zip")),
            ..ToolOptions::default()
        };
        assert!(run(&again, Stop::new()).is_err());
        again.action = ToolAction::Check;
        again.out = None;
        run(&again, Stop::new()).unwrap();
    }

    /// The starter game in a real headless Chrome: every step passes;
    /// a page that throws is caught.
    /// TVPIPE_TEST_CHROME=/path/to/chrome cargo test --release smoke_ -- --ignored
    #[test]
    #[ignore]
    fn smoke_the_template() {
        let Ok(bin) = std::env::var("TVPIPE_TEST_CHROME") else {
            eprintln!("TVPIPE_TEST_CHROME not set");
            return;
        };
        crate::util::timer::raise_resolution();
        let dir = scratch("smoke").join("buzz");
        init(&dir).unwrap();
        let opts = SmokeOptions {
            browser: Some(bin.into()),
            ..SmokeOptions::default()
        };
        let r = smoke::smoke(&dir, &opts).unwrap();
        assert!(r.problems.is_empty(), "{}", r.render());
        assert_eq!(r.passed.len(), 5, "{}", r.render());
        assert!(
            r.passed[4].starts_with("a phone joined room"),
            "{}",
            r.render()
        );
        let tv = dir.join("tv/tv.js");
        let js = std::fs::read_to_string(&tv).unwrap();
        std::fs::write(&tv, format!("{js}\nthrow new Error(\"boom\");\n")).unwrap();
        let r = smoke::smoke(&dir, &opts).unwrap();
        assert!(
            r.problems
                .iter()
                .any(|p| p.at == "tv page" && p.message.contains("uncaught Error: boom")),
            "{}",
            r.render()
        );
    }
}
