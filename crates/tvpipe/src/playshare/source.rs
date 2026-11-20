//! What `tvpipe game play SOURCE` may name (docs/playshare-design.md
//! §18): a local folder or zip (a path that exists wins), a zip by
//! `https://` URL, a GitHub release asset (`gh:OWNER/REPO[/ASSET][@TAG]`,
//! and github.com release URLs rewritten to it), or a name from the games
//! list. Remote sources may carry a pin, `#sha256=HEX`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::net::fetch::Url;
use crate::util::hex;

/// Which release of a GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    /// GitHub's "latest" release.
    Latest,
    /// This tag.
    Exact(String),
    /// The newest release whose tag starts with this (`@drawbluff-v*`) and
    /// that fits this tvpipe.
    Prefix(String),
}

/// A GitHub release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHub {
    pub owner: String,
    pub repo: String,
    /// The asset's file name (`.zip` added when it has no extension);
    /// `None`: the release's only zip, else `REPO.zip`.
    pub asset: Option<String>,
    pub tag: Tag,
}

/// Where a remote game comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    Url(Url),
    GitHub(GitHub),
}

/// A game on the internet, maybe pinned to a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub at: Where,
    pub sha256: Option<[u8; 32]>,
}

/// A parsed `SOURCE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Remote(Remote),
    /// An entry of the games list.
    Name(String),
}

fn bad(s: &str, why: &str) -> Error {
    Error::usage(format!("{s}: {why}"))
}

/// A name in the games list: `[a-z0-9-]`, 1–40, starting with a letter or
/// digit.
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn gh_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn tag_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 200
        && !t.starts_with('/')
        && !t.contains("..")
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'/'))
}

fn asset_ok(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 200
        && !a.starts_with('.')
        && a.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
}

/// `#sha256=HEX` (64 hex digits).
fn parse_pin(s: &str, frag: &str) -> Result<[u8; 32]> {
    let hexs = frag
        .strip_prefix("sha256=")
        .ok_or_else(|| bad(s, "the only fragment tvpipe knows is #sha256=HEX"))?;
    let bytes = hex::decode(hexs).ok().filter(|b| b.len() == 32);
    let bytes = bytes.ok_or_else(|| bad(s, "#sha256= needs 64 hex digits"))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn with_zip(a: &str) -> String {
    if a.contains('.') {
        a.to_owned()
    } else {
        format!("{a}.zip")
    }
}

impl GitHub {
    /// `OWNER/REPO[/ASSET][@TAG]` (after `gh:`).
    fn parse(s: &str, rest: &str) -> Result<GitHub> {
        let (path, tag) = match rest.split_once('@') {
            Some((p, t)) => (p, Some(t)),
            None => (rest, None),
        };
        let parts: Vec<&str> = path.split('/').collect();
        let (owner, repo, asset) = match parts[..] {
            [o, r] => (o, r, None),
            [o, r, a] => (o, r, Some(a)),
            _ => return Err(bad(s, "expected gh:OWNER/REPO[/ASSET][@TAG]")),
        };
        if !gh_part(owner) || !gh_part(repo) {
            return Err(bad(s, "OWNER and REPO are GitHub names"));
        }
        if asset.is_some_and(|a| !asset_ok(a)) {
            return Err(bad(s, "ASSET is a file name like game.zip"));
        }
        let tag = match tag {
            None => Tag::Latest,
            Some(t) => match t.strip_suffix('*') {
                Some(p) if p.is_empty() || tag_ok(p) => Tag::Prefix(p.to_owned()),
                Some(_) => return Err(bad(s, "not a tag prefix")),
                None if tag_ok(t) => Tag::Exact(t.to_owned()),
                None => return Err(bad(s, "not a tag")),
            },
        };
        Ok(GitHub {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            asset: asset.map(with_zip),
            tag,
        })
    }

    /// A github.com URL of a repository or a release: `/O/R`,
    /// `/O/R/releases[/latest]`, `/O/R/releases/tag/T`,
    /// `/O/R/releases/download/T/F`, `/O/R/releases/latest/download/F`.
    fn from_url(s: &str, u: &Url) -> Result<GitHub> {
        let segs: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
        let not = || bad(s, "not a GitHub repository or release URL");
        let (owner, repo) = match segs.as_slice() {
            [o, r, ..] => (*o, r.strip_suffix(".git").unwrap_or(r)),
            _ => return Err(not()),
        };
        let (asset, tag) = match &segs[2..] {
            [] | ["releases"] | ["releases", "latest"] => (None, Tag::Latest),
            ["releases", "tag", t] => (None, Tag::Exact((*t).to_owned())),
            ["releases", "download", t, f] => (Some(*f), Tag::Exact((*t).to_owned())),
            ["releases", "latest", "download", f] => (Some(*f), Tag::Latest),
            _ => return Err(not()),
        };
        let g = GitHub {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            asset: asset.map(with_zip),
            tag,
        };
        // The same checks as the gh: form.
        GitHub::parse(s, &g.to_string()[3..])
    }
}

impl fmt::Display for GitHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gh:{}/{}", self.owner, self.repo)?;
        if let Some(a) = &self.asset {
            write!(f, "/{a}")?;
        }
        match &self.tag {
            Tag::Latest => Ok(()),
            Tag::Exact(t) => write!(f, "@{t}"),
            Tag::Prefix(p) => write!(f, "@{p}*"),
        }
    }
}

impl Remote {
    /// A remote form: `gh:…`, `https://…` (github.com release URLs become
    /// `gh:`), `http://` for loopback or with a pin.
    pub fn parse(s: &str) -> Result<Remote> {
        let (main, frag) = match s.split_once('#') {
            Some((m, f)) => (m, Some(f)),
            None => (s, None),
        };
        let sha256 = frag.map(|f| parse_pin(s, f)).transpose()?;
        let at = if let Some(rest) = main.strip_prefix("gh:") {
            Where::GitHub(GitHub::parse(s, rest)?)
        } else {
            let u = Url::parse(main)?;
            if u.host == "github.com" || u.host == "www.github.com" {
                Where::GitHub(GitHub::from_url(s, &u)?)
            } else {
                if !u.https && !u.is_loopback() && sha256.is_none() {
                    return Err(bad(s, "plain http only with a pin (#sha256=…); use https"));
                }
                Where::Url(u)
            }
        };
        Ok(Remote { at, sha256 })
    }

    /// The source without its pin: the cache's key, what the question
    /// shows.
    pub fn key(&self) -> String {
        match &self.at {
            Where::Url(u) => u.to_string(),
            Where::GitHub(g) => g.to_string(),
        }
    }

    /// With the pin, if any.
    pub fn pinned(&self, sha256: &[u8; 32]) -> String {
        format!("{}#sha256={}", self.key(), hex::encode(sha256))
    }

    /// Whether the same source can give other bytes later (the latest
    /// release, a tag prefix, a URL): asked again each time.
    pub fn moves(&self) -> bool {
        match &self.at {
            Where::Url(_) => true,
            Where::GitHub(g) => !matches!(g.tag, Tag::Exact(_)),
        }
    }
}

impl Source {
    /// Parses `SOURCE`: an existing path first, then the remote forms,
    /// then a name of the games list.
    pub fn parse(s: &str) -> Result<Source> {
        Source::parse_with(s, &|p| p.exists())
    }

    fn parse_with(s: &str, exists: &dyn Fn(&Path) -> bool) -> Result<Source> {
        if s.is_empty() {
            return Err(Error::usage("an empty game"));
        }
        if exists(Path::new(s)) {
            return Ok(Source::Local(PathBuf::from(s)));
        }
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("gh:") || lower.starts_with("https://") || lower.starts_with("http://")
        {
            return Remote::parse(s).map(Source::Remote);
        }
        if s.contains("://") {
            return Err(bad(
                s,
                "only https:// (and gh:) games come from the internet",
            ));
        }
        if is_name(s) {
            return Ok(Source::Name(s.to_owned()));
        }
        Err(Error::not_found(format!(
            "{s}: no such file or folder (a game from the internet is https://…, \
             gh:OWNER/REPO or a name from the games list)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Source> {
        Source::parse_with(s, &|p| {
            p == Path::new("games/drawbluff") || p == Path::new("x.zip")
        })
    }

    fn gh(s: &str) -> GitHub {
        match parse(s).unwrap() {
            Source::Remote(Remote {
                at: Where::GitHub(g),
                ..
            }) => g,
            other => panic!("{s}: {other:?}"),
        }
    }

    #[test]
    fn local_paths_win() {
        assert_eq!(
            parse("games/drawbluff").unwrap(),
            Source::Local("games/drawbluff".into())
        );
        assert_eq!(parse("x.zip").unwrap(), Source::Local("x.zip".into()));
        assert!(parse("y.zip")
            .unwrap_err()
            .message()
            .contains("no such file"));
        assert!(parse("some/folder").is_err());
        // A bare owner/repo is not GitHub.
        assert!(parse("mitsuhiko/tvpipe").is_err());
        assert_eq!(
            parse("drawbluff").unwrap(),
            Source::Name("drawbluff".into())
        );
        assert!(parse("Drawbluff").is_err());
        assert!(parse("ftp://x/y.zip").is_err());
    }

    #[test]
    fn gh_forms() {
        let g = gh("gh:mitsuhiko/tvpipe");
        assert_eq!((g.asset, g.tag), (None, Tag::Latest));
        let g = gh("gh:mitsuhiko/tvpipe/drawbluff@drawbluff-v*");
        assert_eq!(g.asset.as_deref(), Some("drawbluff.zip"));
        assert_eq!(g.tag, Tag::Prefix("drawbluff-v".into()));
        assert_eq!(
            g.to_string(),
            "gh:mitsuhiko/tvpipe/drawbluff.zip@drawbluff-v*"
        );
        let g = gh("gh:o/r/game.zip@v1.2.0");
        assert_eq!(g.tag, Tag::Exact("v1.2.0".into()));
        assert_eq!(gh("gh:o/r@*").tag, Tag::Prefix(String::new()));
        for bad in [
            "gh:o",
            "gh:o/r/a/b",
            "gh:o/../x",
            "gh:o/r@",
            "gh:o/r@a b",
            "gh:o/r/.hidden",
            "gh:o/r@x*y*",
            "gh:o/r#sha256=12",
            "gh:o/r#md5=00",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn github_urls_become_gh() {
        for (url, want) in [
            ("https://github.com/o/r", "gh:o/r"),
            ("https://github.com/o/r.git", "gh:o/r"),
            ("https://github.com/o/r/releases", "gh:o/r"),
            ("https://github.com/o/r/releases/latest", "gh:o/r"),
            ("https://github.com/o/r/releases/tag/v1", "gh:o/r@v1"),
            (
                "https://github.com/o/r/releases/download/v1/g.zip",
                "gh:o/r/g.zip@v1",
            ),
            (
                "https://www.github.com/o/r/releases/latest/download/g.zip",
                "gh:o/r/g.zip",
            ),
        ] {
            assert_eq!(gh(url).to_string(), want, "{url}");
        }
        assert!(parse("https://github.com/o/r/blob/main/x.zip").is_err());
        assert!(parse("https://github.com/").is_err());
    }

    #[test]
    fn urls_and_pins() {
        let pin = "ab".repeat(32);
        let Source::Remote(r) = parse(&format!("https://example.com/g.zip#sha256={pin}")).unwrap()
        else {
            panic!()
        };
        assert_eq!(r.sha256, Some([0xab; 32]));
        assert_eq!(r.key(), "https://example.com/g.zip");
        assert_eq!(
            r.pinned(&[0xab; 32]),
            format!("https://example.com/g.zip#sha256={pin}")
        );
        assert!(r.moves());
        assert!(parse("http://example.com/g.zip")
            .unwrap_err()
            .message()
            .contains("pin"));
        assert!(parse(&format!("http://example.com/g.zip#sha256={pin}")).is_ok());
        assert!(parse("http://127.0.0.1:8000/g.zip").is_ok());
        let Source::Remote(r) = parse("gh:o/r/g@v1").unwrap() else {
            panic!()
        };
        assert!(!r.moves());
        let Source::Remote(r) = parse("gh:o/r/g@v*").unwrap() else {
            panic!()
        };
        assert!(r.moves());
    }
}
