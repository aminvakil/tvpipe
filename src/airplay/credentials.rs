//! Persisted AirPlay pairing state (PLAN §4, §7.1): our controller identity
//! (an Ed25519 key pair plus a pairing identifier, generated once) and the
//! receivers we paired with (their identifier and long-term public key).
//!
//! The store is a small line-based text file, `credentials.txt` in the
//! config directory ([`config_dir`]):
//!
//! ```text
//! # streamlib AirPlay credentials: private, do not share
//! controller = 5B3C1A0E-...-UUID
//! seed = <64 hex digits>
//!
//! receiver = <identifier the receiver sent in pair-setup M6, as hex>
//! pk = <64 hex digits>
//! name = Livingroom (2)
//! deviceid = AA:BB:CC:00:00:0A
//! addr = 192.168.1.34:7000
//! ```
//!
//! Blocks are separated by blank lines; `#` starts a comment line. Unknown
//! keys are ignored, so later versions can add fields. Saving writes a
//! temporary file next to the store and renames it over the old one, with
//! permissions 0600 on unix.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::crypto::ring::{random_array, Ed25519KeyPair, ED25519_PUBLIC_KEY_LEN};
use crate::error::{Error, Result};
use crate::util::hex;

/// File name of the store inside [`config_dir`].
pub const FILE_NAME: &str = "credentials.txt";

/// Overrides the config directory (tests, portable installs).
pub const CONFIG_DIR_ENV: &str = "STREAMLIB_CONFIG_DIR";

/// The platform config directory for streamlib: `$STREAMLIB_CONFIG_DIR`,
/// else `$XDG_CONFIG_HOME/streamlib` (or `~/.config/streamlib`) on unix,
/// `~/Library/Application Support/streamlib` on macOS, and
/// `%APPDATA%\streamlib` on Windows. Computed from environment variables
/// only.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = env_path(CONFIG_DIR_ENV) {
        return Ok(dir);
    }
    platform_config_dir()
        .map(|d| d.join("streamlib"))
        .ok_or_else(|| Error::not_found("credentials: cannot determine the config directory"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    env_path("HOME").map(|h| h.join("Library").join("Application Support"))
}

#[cfg(target_os = "windows")]
fn platform_config_dir() -> Option<PathBuf> {
    env_path("APPDATA")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_config_dir() -> Option<PathBuf> {
    env_path("XDG_CONFIG_HOME")
        .filter(|p| p.is_absolute())
        .or_else(|| env_path("HOME").map(|h| h.join(".config")))
}

/// The default store path: `config_dir()/credentials.txt`.
pub fn default_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(FILE_NAME))
}

/// Our long-term controller identity.
#[derive(Debug)]
pub struct Identity {
    /// The pairing identifier we present (an upper-case UUID).
    pub id: String,
    pub key: Ed25519KeyPair,
}

impl Identity {
    /// A fresh identity: a random Ed25519 key and a random v4 UUID.
    pub fn generate() -> Result<Identity> {
        Ok(Identity {
            id: random_uuid()?,
            key: Ed25519KeyPair::generate()?,
        })
    }

    pub fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
        self.key.public_key()
    }
}

/// A random version 4 UUID, upper case (the form Apple devices use).
pub fn random_uuid() -> Result<String> {
    let mut b: [u8; 16] = random_array()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(&b).to_uppercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

/// A receiver we paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receiver {
    /// The identifier the receiver sent in pair-setup M6 (and signs in
    /// pair-verify M2).
    pub id: Vec<u8>,
    /// Its long-term Ed25519 public key.
    pub public_key: [u8; ED25519_PUBLIC_KEY_LEN],
    /// Display name at pairing time.
    pub name: String,
    /// The TXT `deviceid`, if known.
    pub device_id: Option<String>,
    /// The last address we reached it at (`ip:port`).
    pub addr: Option<String>,
}

impl Receiver {
    /// The identifier for display (lossy UTF-8).
    pub fn id_display(&self) -> String {
        String::from_utf8_lossy(&self.id).into_owned()
    }
}

/// What a lookup knows about the target.
#[derive(Debug, Clone, Default)]
pub struct Query<'a> {
    /// The receiver's TXT `pk` (its long-term key on Apple devices).
    pub public_key: Option<&'a [u8]>,
    pub device_id: Option<&'a str>,
    /// Names the device answers to (case-insensitive).
    pub names: Vec<&'a str>,
    /// `ip` or `ip:port`.
    pub addr: Option<&'a str>,
}

impl Query<'_> {
    fn matches(&self, r: &Receiver) -> bool {
        if let Some(pk) = self.public_key {
            if pk == r.public_key {
                return true;
            }
        }
        if let (Some(want), Some(have)) = (self.device_id, &r.device_id) {
            if want.eq_ignore_ascii_case(have) {
                return true;
            }
        }
        if self
            .names
            .iter()
            .any(|n| n.to_lowercase() == r.name.to_lowercase())
        {
            return true;
        }
        if let (Some(want), Some(have)) = (self.addr, &r.addr) {
            if want == have || have.rsplit_once(':').is_some_and(|(ip, _)| ip == want) {
                return true;
            }
        }
        false
    }
}

/// The credential store, loaded in memory.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    pub identity: Identity,
    pub receivers: Vec<Receiver>,
}

const HEADER: &str = "# streamlib AirPlay credentials: private, do not share\n";

impl Store {
    /// Loads the store at `path`, creating a new identity (and saving it)
    /// if the file does not exist.
    pub fn open(path: &Path) -> Result<Store> {
        match std::fs::read_to_string(path) {
            Ok(text) => Store::parse(path, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let store = Store {
                    path: path.to_owned(),
                    identity: Identity::generate()?,
                    receivers: Vec::new(),
                };
                info!("created controller identity {}", store.identity.id);
                store.save()?;
                Ok(store)
            }
            Err(e) => Err(Error::io(
                format!("credentials: read {}", path.display()),
                e,
            )),
        }
    }

    /// [`open`](Self::open) at [`default_path`].
    pub fn open_default() -> Result<Store> {
        Store::open(&default_path()?)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn find(&self, q: &Query<'_>) -> Option<&Receiver> {
        // Key and device id beat names and addresses.
        let strong = Query {
            public_key: q.public_key,
            device_id: q.device_id,
            ..Query::default()
        };
        self.receivers
            .iter()
            .find(|r| strong.matches(r))
            .or_else(|| self.receivers.iter().find(|r| q.matches(r)))
    }

    /// Adds `r`, replacing any receiver with the same id or public key.
    pub fn upsert(&mut self, r: Receiver) {
        self.receivers
            .retain(|o| o.id != r.id && o.public_key != r.public_key);
        self.receivers.push(r);
    }

    /// Removes the receivers matching `q`; returns them.
    pub fn remove(&mut self, q: &Query<'_>) -> Vec<Receiver> {
        let mut removed = Vec::new();
        while let Some(r) = self.find(q).cloned() {
            self.receivers.retain(|o| *o != r);
            removed.push(r);
        }
        removed
    }

    /// Parses the file format (see the module docs).
    pub fn parse(path: &Path, text: &str) -> Result<Store> {
        let err = |line: usize, msg: &str| {
            Error::malformed(format!("credentials: {}:{line}: {msg}", path.display()))
        };
        let mut identity: Option<Identity> = None;
        let mut receivers = Vec::new();
        for block in blocks(text) {
            let line = block.line;
            let (first, _) = &block.entries[0];
            match first.as_str() {
                "controller" => {
                    if identity.is_some() {
                        return Err(err(line, "more than one controller block"));
                    }
                    let id = block.get("controller").unwrap_or_default().to_owned();
                    if id.is_empty() {
                        return Err(err(line, "empty controller id"));
                    }
                    let seed = block
                        .get("seed")
                        .ok_or_else(|| err(line, "controller without seed"))?;
                    let seed = hex::decode(seed).map_err(|_| err(line, "bad seed hex"))?;
                    let key = Ed25519KeyPair::from_seed_slice(&seed)
                        .map_err(|_| err(line, "seed must be 32 bytes"))?;
                    identity = Some(Identity { id, key });
                }
                "receiver" => {
                    let id = hex::decode(block.get("receiver").unwrap_or_default())
                        .map_err(|_| err(line, "bad receiver id hex"))?;
                    if id.is_empty() {
                        return Err(err(line, "empty receiver id"));
                    }
                    let pk = block
                        .get("pk")
                        .ok_or_else(|| err(line, "receiver without pk"))?;
                    let pk = hex::decode(pk).map_err(|_| err(line, "bad pk hex"))?;
                    let public_key: [u8; ED25519_PUBLIC_KEY_LEN] = pk
                        .as_slice()
                        .try_into()
                        .map_err(|_| err(line, "pk must be 32 bytes"))?;
                    let opt = |k: &str| block.get(k).filter(|v| !v.is_empty()).map(str::to_owned);
                    receivers.push(Receiver {
                        id,
                        public_key,
                        name: block.get("name").unwrap_or_default().to_owned(),
                        device_id: opt("deviceid"),
                        addr: opt("addr"),
                    });
                }
                other => {
                    debug!("credentials: ignoring unknown block {other:?} at line {line}");
                }
            }
        }
        let identity = identity.ok_or_else(|| err(1, "no controller block"))?;
        Ok(Store {
            path: path.to_owned(),
            identity,
            receivers,
        })
    }

    /// The file contents.
    pub fn to_text(&self) -> String {
        let mut out = String::from(HEADER);
        let _ = write!(
            out,
            "controller = {}\nseed = {}\n",
            clean(&self.identity.id),
            hex::encode(self.identity.key.seed())
        );
        for r in &self.receivers {
            let _ = write!(
                out,
                "\nreceiver = {}\npk = {}\nname = {}\n",
                hex::encode(&r.id),
                hex::encode(&r.public_key),
                clean(&r.name)
            );
            if let Some(d) = &r.device_id {
                let _ = writeln!(out, "deviceid = {}", clean(d));
            }
            if let Some(a) = &r.addr {
                let _ = writeln!(out, "addr = {}", clean(a));
            }
        }
        out
    }

    /// Writes the store atomically (temporary file + rename), creating the
    /// directory if needed.
    pub fn save(&self) -> Result<()> {
        write_private(&self.path, self.to_text().as_bytes())
    }
}

/// Values are single-line: control characters become spaces and the value
/// is trimmed, so a hostile mDNS name cannot inject keys.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

struct Block {
    line: usize,
    entries: Vec<(String, String)>,
}

impl Block {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut cur = Block {
        line: 0,
        entries: Vec::new(),
    };
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            if !cur.entries.is_empty() {
                out.push(std::mem::replace(
                    &mut cur,
                    Block {
                        line: 0,
                        entries: Vec::new(),
                    },
                ));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            debug!("credentials: ignoring line {} without '='", idx + 1);
            continue;
        };
        if cur.entries.is_empty() {
            cur.line = idx + 1;
        }
        cur.entries.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    if !cur.entries.is_empty() {
        out.push(cur);
    }
    out
}

/// Writes `data` to `path` via a temporary file in the same directory and
/// a rename. The file is created with mode 0600 on unix.
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::io(format!("credentials: create {}", dir.display()), e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| FILE_NAME.to_owned());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let ctx = |what: &str| format!("credentials: {what} {}", tmp.display());
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp).map_err(|e| Error::io(ctx("create"), e))?;
        f.write_all(data).map_err(|e| Error::io(ctx("write"), e))?;
        f.sync_all().map_err(|e| Error::io(ctx("sync"), e))?;
        drop(f);
        std::fs::rename(&tmp, path)
            .map_err(|e| Error::io(format!("credentials: rename to {}", path.display()), e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh, empty directory under the target dir for one test.
    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn receiver(n: u8) -> Receiver {
        Receiver {
            id: format!("receiver-{n}").into_bytes(),
            public_key: [n; 32],
            name: format!("TV {n}"),
            device_id: Some(format!("AA:BB:CC:00:00:{n:02X}")),
            addr: Some(format!("192.168.1.{n}:7000")),
        }
    }

    #[test]
    fn create_save_reload() {
        let dir = temp_dir("credentials-roundtrip");
        let path = dir.join("sub").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        assert!(path.exists());
        assert_eq!(store.identity.id.len(), 36);
        store.upsert(receiver(1));
        store.upsert(receiver(2));
        let mut evil = receiver(3);
        evil.name = "line\nreceiver = 00\r".into();
        store.upsert(evil);
        store.save().unwrap();

        let again = Store::open(&path).unwrap();
        assert_eq!(again.identity.id, store.identity.id);
        assert_eq!(again.identity.public_key(), store.identity.public_key());
        assert_eq!(again.receivers.len(), 3);
        assert_eq!(again.receivers[0], receiver(1));
        assert_eq!(again.receivers[2].name, "line receiver = 00");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No temporary files left behind.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(FILE_NAME)]);
    }

    #[test]
    fn find_upsert_remove() {
        let mut store = Store {
            path: PathBuf::from("x"),
            identity: Identity::generate().unwrap(),
            receivers: vec![receiver(1), receiver(2)],
        };
        let by_pk = Query {
            public_key: Some(&[2; 32]),
            names: vec!["TV 1"],
            ..Query::default()
        };
        assert_eq!(store.find(&by_pk).unwrap().id, b"receiver-2");
        let by_name = Query {
            names: vec!["tv 1"],
            ..Query::default()
        };
        assert_eq!(store.find(&by_name).unwrap().id, b"receiver-1");
        let by_addr = Query {
            addr: Some("192.168.1.2"),
            ..Query::default()
        };
        assert_eq!(store.find(&by_addr).unwrap().id, b"receiver-2");
        let by_dev = Query {
            device_id: Some("aa:bb:cc:00:00:01"),
            ..Query::default()
        };
        assert_eq!(store.find(&by_dev).unwrap().id, b"receiver-1");
        assert!(store
            .find(&Query {
                names: vec!["nope"],
                ..Query::default()
            })
            .is_none());

        let mut newer = receiver(1);
        newer.name = "Renamed".into();
        store.upsert(newer);
        assert_eq!(store.receivers.len(), 2);
        assert_eq!(store.receivers[1].name, "Renamed");

        let removed = store.remove(&by_dev);
        assert_eq!(removed.len(), 1);
        assert_eq!(store.receivers.len(), 1);
    }

    #[test]
    fn parse_errors_are_errors() {
        let p = Path::new("c.txt");
        assert!(Store::parse(p, "").is_err());
        assert!(Store::parse(p, "controller = X\nseed = zz\n").is_err());
        assert!(Store::parse(p, "controller = X\nseed = 0011\n").is_err());
        let seed = "00".repeat(32);
        let ok = format!("controller = X\nseed = {seed}\n");
        assert!(Store::parse(p, &ok).is_ok());
        let bad_pk = format!("{ok}\nreceiver = 01\npk = 0102\n");
        assert!(Store::parse(p, &bad_pk).is_err());
        let unknown = format!("# c\n{ok}\nfuture = 1\nx = y\n\ngarbage line\n");
        assert!(Store::parse(p, &unknown).is_ok());
    }

    #[test]
    fn uuid_shape() {
        let u = random_uuid().unwrap();
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert_eq!(&parts[2][..1], "4");
        assert!("89AB".contains(&parts[3][..1]));
    }
}
