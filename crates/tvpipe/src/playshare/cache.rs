//! The cache of games fetched from the internet (docs/playshare-design.md
//! §18): `games/blobs/SHA256.zip` (content-addressed), `games/index.json`
//! (what each source gave last, and which hash the person approved) and
//! `games/list.json` (the games list) in the platform's cache directory.
//!
//! Writes go through a temporary file and a rename; two tvpipes writing at
//! once lose one's index update, never a blob.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::codec::json::Value;
use crate::error::{Error, Result};

/// What one source gave last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    /// The download URL it resolved to (unsigned, before redirects).
    pub url: String,
    /// The release tag (GitHub).
    pub tag: Option<String>,
    /// The download's ETag, for `If-None-Match`.
    pub etag: Option<String>,
    /// Hex.
    pub sha256: String,
    pub size: u64,
    pub name: String,
    pub title: String,
    /// The manifest's `tvpipe`.
    pub series: Option<String>,
    /// Unix seconds.
    pub fetched: u64,
    pub used: u64,
    /// The hash the person said yes to (`game play`'s question).
    pub trusted: Option<String>,
}

/// `index.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    /// By source (`Remote::key`).
    pub sources: BTreeMap<String, Record>,
    /// The games list's URL and ETag when it was fetched.
    pub list: Option<(String, Option<String>)>,
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

impl Record {
    fn to_value(&self) -> Value {
        Value::object()
            .with("url", self.url.as_str())
            .with_opt("tag", self.tag.as_deref())
            .with_opt("etag", self.etag.as_deref())
            .with("sha256", self.sha256.as_str())
            .with("size", self.size)
            .with("name", self.name.as_str())
            .with("title", self.title.as_str())
            .with_opt("series", self.series.as_deref())
            .with("fetched", self.fetched)
            .with("used", self.used)
            .with_opt("trusted", self.trusted.as_deref())
    }

    fn from_value(v: &Value) -> Option<Record> {
        let sha256 = opt(v, "sha256").filter(|s| is_hex64(s))?;
        Some(Record {
            url: opt(v, "url")?,
            tag: opt(v, "tag"),
            etag: opt(v, "etag"),
            sha256,
            size: v.get("size").and_then(Value::as_u64).unwrap_or(0),
            name: opt(v, "name").unwrap_or_default(),
            title: opt(v, "title").unwrap_or_default(),
            series: opt(v, "series"),
            fetched: v.get("fetched").and_then(Value::as_u64).unwrap_or(0),
            used: v.get("used").and_then(Value::as_u64).unwrap_or(0),
            trusted: opt(v, "trusted").filter(|s| is_hex64(s)),
        })
    }
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Index {
    pub fn parse(text: &str) -> Result<Index> {
        let v = Value::parse(text)?;
        let mut idx = Index::default();
        if let Some(o) = v.get("sources").and_then(Value::as_object) {
            for (k, r) in o.iter() {
                if let Some(r) = Record::from_value(r) {
                    idx.sources.insert(k.to_owned(), r);
                }
            }
        }
        if let Some(l) = v.get("list") {
            if let Some(url) = opt(l, "url") {
                idx.list = Some((url, opt(l, "etag")));
            }
        }
        Ok(idx)
    }

    pub fn to_json(&self) -> String {
        let sources: crate::codec::json::Object = self
            .sources
            .iter()
            .map(|(k, r)| (k.clone(), r.to_value()))
            .collect();
        let mut v = Value::object().with("v", 1u64).with("sources", sources);
        if let Some((url, etag)) = &self.list {
            v = v.with(
                "list",
                Value::object()
                    .with("url", url.as_str())
                    .with_opt("etag", etag.as_deref()),
            );
        }
        v.to_json_pretty() + "\n"
    }
}

/// The cache directory of games.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

/// Seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Cache {
    /// `games/` in the platform's cache directory (`$TVPIPE_CACHE_DIR`).
    pub fn open() -> Result<Cache> {
        Ok(Cache::at(crate::storage::cache_dir()?.join("games")))
    }

    pub fn at(dir: impl Into<PathBuf>) -> Cache {
        Cache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn mkdir(&self, d: &Path) -> Result<()> {
        std::fs::create_dir_all(d).map_err(|e| Error::io(format!("cache: {}", d.display()), e))
    }

    /// The index; an unreadable one counts as empty (a warning).
    pub fn index(&self) -> Index {
        let p = self.dir.join("index.json");
        match std::fs::read_to_string(&p) {
            Ok(t) => Index::parse(&t).unwrap_or_else(|e| {
                warn!("cache: {}: {e}; starting afresh", p.display());
                Index::default()
            }),
            Err(_) => Index::default(),
        }
    }

    pub fn save(&self, idx: &Index) -> Result<()> {
        self.write("index.json", idx.to_json().as_bytes())
    }

    /// Writes `name` in the cache directory atomically.
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.mkdir(&self.dir)?;
        let p = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &p))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::io(format!("cache: write {}", p.display()), e)
            })
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(name)).ok()
    }

    /// The blob of a hash (hex), if it is there.
    pub fn blob(&self, sha256: &str) -> Option<PathBuf> {
        let p = self.blob_path(sha256);
        p.is_file().then_some(p)
    }

    fn blob_path(&self, sha256: &str) -> PathBuf {
        self.dir.join("blobs").join(format!("{sha256}.zip"))
    }

    /// A new temporary file for a download.
    pub fn temp(&self) -> Result<(PathBuf, File)> {
        let d = self.dir.join("blobs");
        self.mkdir(&d)?;
        let rand = crate::util::hex::encode(&crate::crypto::ring::random_array::<8>()?);
        let p = d.join(format!(".download-{rand}.tmp"));
        let f = File::create(&p).map_err(|e| Error::io(format!("cache: {}", p.display()), e))?;
        Ok((p, f))
    }

    /// Moves a finished download in as the blob of its hash.
    pub fn keep(&self, temp: &Path, sha256: &str) -> Result<PathBuf> {
        let p = self.blob_path(sha256);
        std::fs::rename(temp, &p).map_err(|e| {
            let _ = std::fs::remove_file(temp);
            Error::io(format!("cache: {}", p.display()), e)
        })?;
        Ok(p)
    }

    /// Deletes blobs no source refers to (and leftover downloads older
    /// than a day).
    pub fn prune(&self, idx: &Index) {
        let Ok(rd) = std::fs::read_dir(self.dir.join("blobs")) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let keep = if let Some(h) = name.strip_suffix(".zip") {
                idx.sources.values().any(|r| r.sha256 == h)
            } else {
                // A download in progress elsewhere is younger.
                e.metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_none_or(|age| age.as_secs() < 86_400)
            };
            if !keep {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sha: &str) -> Record {
        Record {
            url: "https://github.com/o/r/releases/download/g-v1/g.zip".into(),
            tag: Some("g-v1".into()),
            etag: Some("\"0x8D\"".into()),
            sha256: sha.into(),
            size: 1234,
            name: "g".into(),
            title: "G".into(),
            series: Some("0.2".into()),
            fetched: 1,
            used: 2,
            trusted: None,
        }
    }

    #[test]
    fn index_round_trips() {
        let mut idx = Index::default();
        idx.sources
            .insert("gh:o/r/g.zip@g-v*".into(), record(&"a".repeat(64)));
        let mut r = record(&"b".repeat(64));
        r.trusted = Some("b".repeat(64));
        r.tag = None;
        idx.sources.insert("https://example.com/g.zip".into(), r);
        idx.list = Some(("https://x/games.json".into(), Some("W/\"1\"".into())));
        let back = Index::parse(&idx.to_json()).unwrap();
        assert_eq!(back, idx);
        // Broken records are dropped, not fatal.
        let back = Index::parse(r#"{"sources":{"a":{"url":"u","sha256":"zz"},"b":7}}"#).unwrap();
        assert!(back.sources.is_empty());
        assert!(Index::parse("not json").is_err());
    }

    #[test]
    fn blobs_keep_and_prune() {
        let dir = crate::storage::cache_dir()
            .unwrap()
            .join("cache-test-blobs");
        let _ = std::fs::remove_dir_all(&dir);
        let c = Cache::at(&dir);
        assert_eq!(c.index(), Index::default());
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        for h in [&a, &b] {
            let (tmp, mut f) = c.temp().unwrap();
            std::io::Write::write_all(&mut f, h.as_bytes()).unwrap();
            drop(f);
            c.keep(&tmp, h).unwrap();
        }
        assert!(c.blob(&a).is_some() && c.blob(&b).is_some());
        let mut idx = Index::default();
        idx.sources.insert("s".into(), record(&a));
        c.save(&idx).unwrap();
        assert_eq!(c.index(), idx);
        c.prune(&idx);
        assert!(c.blob(&a).is_some());
        assert!(c.blob(&b).is_none());
        std::fs::write(dir.join("index.json"), "garbage").unwrap();
        assert_eq!(c.index(), Index::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
