//! `streamlib pair` / `unpair` (PLAN §4) and [`connect`], the
//! authenticated control connection the mirroring sink (W7) starts from.
//!
//! `connect` opens the control port, pair-verifies with stored
//! credentials (or does transient pair-setup when the receiver allows
//! it), and switches the connection to the encrypted channel. The
//! returned [`Control`] carries the RTSP client and the shared secret.

use std::io::{BufRead, Write as _};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::airplay::credentials::{Identity, Query, Receiver, Store};
use crate::airplay::info::get_info;
use crate::airplay::pairing::{self, Shared};
use crate::airplay::secure::SecureStream;
use crate::error::{Error, ErrorKind, Result};
use crate::net::device::Device;
use crate::net::discover;
use crate::net::http::Conn;
use crate::net::rtsp::RtspClient;

/// The AirPlay control port when only an address is given.
pub const DEFAULT_PORT: u16 = 7000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(4);

/// The encrypted control connection.
pub type SecureClient = RtspClient<SecureStream<TcpStream>>;

/// How [`connect`] authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// Pair-verify with stored credentials.
    Verified(Receiver),
    /// Transient pair-setup (no stored credentials needed).
    Transient,
}

/// An authenticated, encrypted control connection.
pub struct Control {
    pub client: SecureClient,
    /// The pairing's shared secret (W7 derives stream keys from it).
    pub shared: Shared,
    pub addr: SocketAddr,
    pub auth: Auth,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Control")
            .field("addr", &self.addr)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

/// A resolved `NAME|ADDR`.
#[derive(Debug, Clone)]
pub struct Target {
    pub addr: SocketAddr,
    /// The discovered device, unless an address was given.
    pub device: Option<Device>,
}

impl Target {
    /// The device's display name, else the address.
    pub fn label(&self) -> String {
        self.device
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| self.addr.to_string())
    }

    /// A credentials query from what discovery knows.
    pub fn query(&self) -> Query<'_> {
        let mut q = Query::default();
        if let Some(d) = &self.device {
            let txt = d.airplay.as_ref().or(d.raop.as_ref()).map(|a| &a.txt);
            q.public_key = txt.and_then(|t| t.pk.as_deref());
            q.device_id = txt.and_then(|t| t.deviceid.as_deref());
            q.names = d.names();
        }
        q
    }
}

/// Parses `ip` or `ip:port` (IPv6 as `[addr]:port`).
fn parse_addr(s: &str) -> Option<SocketAddr> {
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, DEFAULT_PORT));
    }
    s.parse::<SocketAddr>().ok()
}

/// Resolves `NAME|ADDR` to the AirPlay control address.
pub fn resolve(target: &str) -> Result<Target> {
    if let Some(addr) = parse_addr(target) {
        return Ok(Target { addr, device: None });
    }
    let device = discover::resolve(target, DISCOVER_TIMEOUT, &[])?
        .ok_or_else(|| Error::not_found(format!("no receiver named {target:?}")))?;
    let ep = device
        .airplay
        .as_ref()
        .or(device.raop.as_ref())
        .map(|a| &a.endpoint)
        .ok_or_else(|| Error::unsupported(format!("{} does not speak AirPlay", device.name)))?;
    let addr = SocketAddr::new(ep.addr(), ep.port);
    debug!("resolved {target:?} to {} at {addr}", device.name);
    Ok(Target {
        addr,
        device: Some(device),
    })
}

/// Opens a plain control connection with timeouts set.
pub fn open(addr: SocketAddr) -> Result<RtspClient<TcpStream>> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| Error::io(format!("airplay: connect to {addr}"), e))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .and_then(|()| stream.set_nodelay(true))
        .map_err(|e| Error::io("airplay: socket options", e))?;
    Ok(RtspClient::new(stream))
}

/// Switches a plain connection to the encrypted channel keyed by `shared`.
pub fn encrypt(mut client: RtspClient<TcpStream>, shared: &Shared) -> Result<SecureClient> {
    if !client.conn().buffered().is_empty() {
        return Err(Error::protocol(
            "airplay: receiver sent data before the encrypted channel started",
        ));
    }
    let stream = client
        .conn()
        .get_ref()
        .try_clone()
        .map_err(|e| Error::io("airplay: clone socket", e))?;
    let secure = SecureStream::new(stream, &shared.control_keys()?);
    Ok(client.replace_conn(Conn::new(secure)))
}

/// Connects and authenticates: pair-verify with `receiver`, else
/// transient pair-setup.
pub fn connect_with(addr: SocketAddr, identity: &Identity, auth: Auth) -> Result<Control> {
    let mut client = open(addr)?;
    let shared = match &auth {
        Auth::Verified(r) => {
            debug!("pair-verify with {} at {addr}", r.name);
            pairing::pair_verify(&mut client, identity, &r.id, &r.public_key)?
        }
        Auth::Transient => {
            debug!("transient pair-setup with {addr}");
            pairing::pair_setup_transient(&mut client)?
        }
    };
    let client = encrypt(client, &shared)?;
    Ok(Control {
        client,
        shared,
        addr,
        auth,
    })
}

/// Connects to a resolved target with the credentials in `store`: stored
/// pairing first, transient pair-setup otherwise.
pub fn connect(target: &Target, store: &Store) -> Result<Control> {
    match store.find(&target.query()).or_else(|| {
        let ip = target.addr.ip().to_string();
        store.find(&Query {
            addr: Some(&ip),
            ..Query::default()
        })
    }) {
        Some(r) => connect_with(target.addr, &store.identity, Auth::Verified(r.clone())),
        None => connect_with(target.addr, &store.identity, Auth::Transient).map_err(|e| {
            if e.kind() == ErrorKind::Auth {
                Error::auth(format!(
                    "{} requires pairing: run `streamlib pair {:?}` ({})",
                    target.label(),
                    target.label(),
                    e.message()
                ))
            } else {
                e
            }
        }),
    }
}

/// Reads the PIN from stdin after prompting on stderr.
fn prompt_pin(label: &str) -> Result<String> {
    eprint!("Enter the PIN shown on {label}: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::io("read PIN", e))?;
    if line.is_empty() {
        return Err(Error::usage("no PIN entered"));
    }
    Ok(line.trim().to_owned())
}

/// Pairs with `target` and stores the credentials in `store`. With `pin`
/// given, the receiver is assumed to have a fixed PIN and
/// `/pair-pin-start` is skipped; without it, the TV shows a PIN and
/// `get_pin` is asked for it.
pub fn pair_with(
    target: &Target,
    store: &mut Store,
    pin: Option<&str>,
    get_pin: impl FnOnce() -> Result<String>,
) -> Result<Receiver> {
    let mut client = open(target.addr)?;
    let plain_info = get_info(&mut client)
        .map_err(|e| debug!("pair: plain /info failed: {e}"))
        .ok();
    let receiver = match pin {
        Some(pin) => {
            let pin = pin.to_owned();
            pairing::pair_setup_pin(&mut client, &store.identity, false, move || Ok(pin))?
        }
        None => pairing::pair_setup_pin(&mut client, &store.identity, true, get_pin)?,
    };
    drop(client);
    if let Some(pk) = plain_info.as_ref().and_then(|i| i.pk.as_deref()) {
        if pk != receiver.public_key {
            debug!("pair: /info pk differs from the pair-setup key");
        }
    }
    let name = target
        .device
        .as_ref()
        .map(|d| d.name.clone())
        .or_else(|| plain_info.as_ref().and_then(|i| i.name.clone()))
        .unwrap_or_else(|| target.addr.ip().to_string());
    let device_id = target
        .device
        .as_ref()
        .and_then(|d| d.airplay.as_ref().or(d.raop.as_ref()))
        .and_then(|a| a.txt.deviceid.clone())
        .or_else(|| plain_info.as_ref().and_then(|i| i.device_id.clone()));
    let record = Receiver {
        id: receiver.id,
        public_key: receiver.public_key,
        name,
        device_id,
        addr: Some(target.addr.to_string()),
    };
    store.upsert(record.clone());
    store.save()?;
    Ok(record)
}

/// The `pair` command body.
pub fn pair(target: &str, pin: Option<&str>) -> Result<()> {
    let target = resolve(target)?;
    let mut store = Store::open_default()?;
    let label = target.label();
    info!("pairing with {label} at {}", target.addr);
    let record = pair_with(&target, &mut store, pin, || prompt_pin(&label))?;
    info!(
        "paired with {} (receiver id {}); credentials in {}",
        record.name,
        record.id_display(),
        store.path().display()
    );
    // Prove the pairing works: verify on a fresh connection and fetch the
    // full /info over the encrypted channel.
    let mut control = connect_with(target.addr, &store.identity, Auth::Verified(record))?;
    let info = get_info(&mut control.client)?;
    let caps = info.capabilities();
    println!(
        "Paired with {} ({}), display {}x{}@{}",
        label,
        info.model.as_deref().unwrap_or("unknown model"),
        caps.max_width,
        caps.max_height,
        caps.max_fps
    );
    Ok(())
}

/// The `unpair` command body: forgets the stored pairing.
pub fn unpair(target: &str) -> Result<()> {
    let mut store = Store::open_default()?;
    let removed = forget(&mut store, target, resolve)?;
    for r in &removed {
        println!("Forgot pairing with {} ({})", r.name, r.id_display());
    }
    Ok(())
}

/// Removes the pairings for `target` from `store` and saves it. Looks the
/// target up by name or address first and only calls `resolve` (which
/// browses) when that finds nothing.
pub fn forget(
    store: &mut Store,
    target: &str,
    resolve: impl FnOnce(&str) -> Result<Target>,
) -> Result<Vec<Receiver>> {
    let addr = parse_addr(target).map(|a| a.ip().to_string());
    let local = Query {
        names: if addr.is_none() { vec![target] } else { vec![] },
        addr: addr.as_deref(),
        ..Query::default()
    };
    let mut removed = store.remove(&local);
    if removed.is_empty() && addr.is_none() {
        match resolve(target) {
            Ok(t) => removed = store.remove(&t.query()),
            Err(e) => debug!("unpair: cannot resolve {target:?}: {e}"),
        }
    }
    if removed.is_empty() {
        return Err(Error::not_found(format!(
            "no stored pairing for {target:?}"
        )));
    }
    store.save()?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airplay::credentials::tests::temp_dir;
    use crate::airplay::credentials::FILE_NAME;
    use crate::airplay::pairing::fake::{Config, FakeReceiver};

    fn target(fake: &FakeReceiver) -> Target {
        Target {
            addr: fake.addr,
            device: None,
        }
    }

    #[test]
    fn pin_pair_then_verify_after_restart() {
        let fake = FakeReceiver::start(Config {
            allow_transient: false,
            ..Config::default()
        });
        let path = temp_dir("pair-pin").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        let record = pair_with(&target(&fake), &mut store, None, || Ok("1234".into())).unwrap();
        assert_eq!(record.id, fake.config.id);
        assert_eq!(record.public_key, fake.public_key());
        assert_eq!(record.name, "Fake TV");
        assert_eq!(record.device_id.as_deref(), Some("AA:BB:CC:00:00:0A"));
        assert_eq!(fake.state.lock().unwrap().pin_starts, 1);
        drop(store);

        // A later run: credentials come from disk.
        let store = Store::open(&path).unwrap();
        assert_eq!(store.receivers.len(), 1);
        let mut control = connect(&target(&fake), &store).unwrap();
        assert!(matches!(control.auth, Auth::Verified(_)));
        assert_eq!(control.shared.secret().len(), 32);
        let info = get_info(&mut control.client).unwrap();
        let caps = info.capabilities();
        assert_eq!((caps.max_width, caps.max_height), (3840, 2160));
        assert_eq!(caps.max_fps.num, 60);
        // A second request on the same encrypted connection.
        let resp = control.client.request("OPTIONS", "*", &[], None).unwrap();
        assert_eq!(resp.status, 404);
        let log = fake.log();
        assert!(log.contains(&"secure GET /info".to_string()), "{log:?}");
        assert!(log.contains(&"secure OPTIONS *".to_string()), "{log:?}");
    }

    #[test]
    fn fixed_pin_skips_pin_start() {
        let fake = FakeReceiver::start(Config::default());
        let path = temp_dir("pair-fixed-pin").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        pair_with(&target(&fake), &mut store, Some("1234"), || {
            panic!("not asked")
        })
        .unwrap();
        assert_eq!(fake.state.lock().unwrap().pin_starts, 0);
    }

    #[test]
    fn wrong_pin_is_auth_error() {
        let fake = FakeReceiver::start(Config::default());
        let path = temp_dir("pair-wrong-pin").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        let err = pair_with(&target(&fake), &mut store, None, || Ok("9999".into())).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth, "{err}");
        assert!(store.receivers.is_empty());
    }

    #[test]
    fn transient_when_unpaired() {
        let fake = FakeReceiver::start(Config::default());
        let path = temp_dir("pair-transient").join(FILE_NAME);
        let store = Store::open(&path).unwrap();
        let mut control = connect(&target(&fake), &store).unwrap();
        assert_eq!(control.auth, Auth::Transient);
        assert_eq!(control.shared.secret().len(), 64);
        let info = get_info(&mut control.client).unwrap();
        assert_eq!(info.displays.len(), 1);
        assert_eq!(
            fake.log(),
            [
                "plain POST /pair-setup",
                "plain POST /pair-setup",
                "secure GET /info"
            ]
        );
    }

    #[test]
    fn unpaired_and_no_transient_says_pair() {
        let fake = FakeReceiver::start(Config {
            allow_transient: false,
            ..Config::default()
        });
        let path = temp_dir("pair-required").join(FILE_NAME);
        let store = Store::open(&path).unwrap();
        let err = connect(&target(&fake), &store).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth);
        assert!(err.message().contains("streamlib pair"), "{err}");
    }

    #[test]
    fn forget_by_name_address_or_discovery() {
        let fake = FakeReceiver::start(Config::default());
        let path = temp_dir("pair-forget").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        pair_with(&target(&fake), &mut store, None, || Ok("1234".into())).unwrap();
        let no_browse = |_: &str| -> Result<Target> { panic!("should not browse") };
        let removed = forget(&mut store, "fake tv", no_browse).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(Store::open(&path).unwrap().receivers.is_empty());

        pair_with(&target(&fake), &mut store, None, || Ok("1234".into())).unwrap();
        let ip = fake.addr.ip().to_string();
        assert_eq!(forget(&mut store, &ip, no_browse).unwrap().len(), 1);

        // Unknown name: falls back to discovery, then reports not found.
        let err = forget(&mut store, "Other", |_| Err(Error::not_found("x"))).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn repeated_verify_and_repair() {
        let fake = FakeReceiver::start(Config::default());
        let path = temp_dir("pair-repeat").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        pair_with(&target(&fake), &mut store, None, || Ok("1234".into())).unwrap();
        let mut secrets = Vec::new();
        for _ in 0..3 {
            let mut c = connect(&target(&fake), &store).unwrap();
            assert!(matches!(c.auth, Auth::Verified(_)));
            get_info(&mut c.client).unwrap();
            secrets.push(c.shared.secret().to_vec());
        }
        // Fresh ephemeral keys every time.
        assert_ne!(secrets[0], secrets[1]);
        // Pairing again replaces the record instead of adding one.
        pair_with(&target(&fake), &mut store, None, || Ok("1234".into())).unwrap();
        assert_eq!(Store::open(&path).unwrap().receivers.len(), 1);
    }

    #[test]
    fn verify_fails_when_receiver_forgot_us() {
        let fake = FakeReceiver::start(Config::default());
        let path = temp_dir("pair-forgotten").join(FILE_NAME);
        let mut store = Store::open(&path).unwrap();
        pair_with(&target(&fake), &mut store, None, || Ok("1234".into())).unwrap();
        fake.state.lock().unwrap().pairings.clear();
        let err = connect(&target(&fake), &store).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth, "{err}");
        assert!(err.message().contains("re-pair"), "{err}");

        // A stored key that does not match the receiver fails in M2.
        store.receivers[0].public_key = [1; 32];
        let err = connect(&target(&fake), &store).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth, "{err}");
    }
}

/// Hardware checks against a real receiver, run by hand (they use the
/// real credential store, like the CLI):
///
/// ```text
/// STREAMLIB_ATV=192.168.1.34 cargo test hardware_pair -- --ignored --nocapture
/// STREAMLIB_ATV=192.168.1.34 cargo test hardware_verify -- --ignored --nocapture
/// ```
#[cfg(test)]
mod hardware {
    use super::*;

    fn atv() -> Option<Target> {
        let addr = parse_addr(&std::env::var("STREAMLIB_ATV").ok()?)?;
        crate::log::set_level(crate::log::Level::Trace);
        Some(Target { addr, device: None })
    }

    #[test]
    #[ignore]
    fn hardware_plain_info() {
        let Some(t) = atv() else { return };
        let mut c = open(t.addr).unwrap();
        let info = get_info(&mut c).unwrap();
        eprintln!("{info:#?}");
    }

    /// Interactive: reads the PIN shown on the TV from stdin.
    #[test]
    #[ignore]
    fn hardware_pair() {
        let Some(t) = atv() else { return };
        let mut store = Store::open_default().unwrap();
        let r = pair_with(&t, &mut store, None, || prompt_pin("the TV")).unwrap();
        eprintln!("paired: {} ({})", r.name, r.id_display());
    }

    /// Pair-verify with the stored credentials, then `/info` over the
    /// encrypted channel; saves the body to `target/appletv-info-verified.bplist`
    /// (scrub it with `tests/fixtures/airplay/scrub_info.py` before committing).
    #[test]
    #[ignore]
    fn hardware_verify() {
        let Some(t) = atv() else { return };
        let store = Store::open_default().unwrap();
        let mut control = connect(&t, &store).unwrap();
        eprintln!(
            "verified with {} at {}",
            match &control.auth {
                Auth::Verified(r) => r.name.as_str(),
                Auth::Transient => "(transient)",
            },
            control.addr
        );
        let resp = control.client.request("GET", "/info", &[], None).unwrap();
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("appletv-info-verified.bplist");
        std::fs::write(&out, &resp.body).unwrap();
        let info = crate::airplay::info::Info::parse(&resp.body).unwrap();
        eprintln!("{:#?}", info.capabilities());
        eprintln!("saved {}", out.display());
    }
}
