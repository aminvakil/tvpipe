//! `tvpipe bundle`: check a game, pack it into a zip, run it once
//! without TVs, or start a new one from the template (docs/games.md).

use std::path::{Path, PathBuf};

use super::check::{self, Checked};
use super::smoke::{self, SmokeOptions};
use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::util::stop::Stop;

/// `tvpipe bundle`'s options.
#[derive(Debug, Clone, Default)]
pub struct BundleOptions {
    /// The game: a folder (packed unless `check`) or a `.zip` (checked).
    pub path: PathBuf,
    /// `-o FILE`: the zip (default `NAME.zip` here).
    pub out: Option<PathBuf>,
    /// `--check`: write nothing.
    pub check: bool,
    /// `--smoke`: also run it once in a headless browser.
    pub smoke: bool,
    /// `--strict`: warnings fail too.
    pub strict: bool,
    /// `--init`: make a starter game in `path`.
    pub init: bool,
    /// `--browser`: for `--smoke`.
    pub browser: Option<PathBuf>,
}

/// Runs `tvpipe bundle`; the report goes to stdout. An error when the
/// game has errors (or warnings with `strict`).
pub fn run(o: &BundleOptions, stop: Stop) -> Result<()> {
    if o.init {
        let files = init(&o.path)?;
        let d = o.path.display();
        println!("made a starter game in {d} ({} files)", files.len());
        println!();
        println!("  tvpipe playshare {d}            play it (edit a file, reload the page)");
        println!("  tvpipe bundle --check {d}       check it");
        println!("  tvpipe bundle {d}               pack it into a zip for others");
        return Ok(());
    }
    let mut c = check::check(&o.path)?;
    let pack = !o.check && !c.is_zip;
    let out = if pack {
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
        if o.out.is_some() && c.is_zip {
            return Err(Error::usage(
                "bundle: a .zip is checked, not packed again (-o is for folders)",
            ));
        }
        None
    };
    print!("{}", c.render());
    fail_on(&c, o.strict)?;
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
            out.clone()
        }
        None => o.path.clone(),
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
                "bundle: the smoke run found {} errors and {} warnings",
                r.errors(),
                warnings
            )));
        }
        println!("smoke: ok");
    }
    Ok(())
}

fn fail_on(c: &Checked, strict: bool) -> Result<()> {
    if c.errors() > 0 {
        return Err(Error::usage(format!(
            "bundle: {} has {} error{}{}",
            c.origin.display(),
            c.errors(),
            if c.errors() == 1 { "" } else { "s" },
            if c.is_zip { "" } else { "; nothing written" }
        )));
    }
    if strict && c.warnings() > 0 {
        return Err(Error::usage(format!(
            "bundle: {} has {} warnings (--strict)",
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
                "bundle --init: {} is not empty",
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
            r if r.ends_with(".html") => html_escape(&title),
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
        let o = BundleOptions {
            path: dir.clone(),
            out: Some(dir.join("quiz-night.zip")),
            ..BundleOptions::default()
        };
        run(&o, Stop::new()).unwrap();
        run(&o, Stop::new()).unwrap();
        let z = check::check(&dir.join("quiz-night.zip")).unwrap();
        assert!(z.problems.is_empty(), "{}", z.render());
        assert!(!z.paths().any(|p| p.ends_with(".zip")));
        // A zip is not packed again.
        let again = BundleOptions {
            path: dir.join("quiz-night.zip"),
            out: Some(dir.join("x.zip")),
            ..BundleOptions::default()
        };
        assert!(run(&again, Stop::new()).is_err());
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
