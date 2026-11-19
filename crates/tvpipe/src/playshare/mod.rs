//! `tvpipe playshare`: web games served by tvpipe
//! (docs/playshare-design.md). A bundle (a folder or a zip with
//! `tvpipe.json`) is served on two listeners; the game's host page runs
//! in a window of tvpipe's own browser and talks to tvpipe over a local
//! API; phones' WebSockets are relayed to it; its TV page is rendered by a
//! headless browser and cast with the `share URL` machinery.
//!
//! - [`bundle`]: the manifest and the files (folders, zips).
//! - [`server`]: the listeners, their access rules, static files.
//! - [`hub`]: the API, the relay, discovery/pairing/cast progress.
//! - [`conn`]: one WebSocket with an outbox and limits.
//! - [`backend`]: discovery, pairing and casting (fakes in tests).
//! - [`window`]: the app window.
//! - [`web`]: the library and the launcher, embedded.

pub mod backend;
pub mod bundle;
pub mod conn;
pub mod hub;
pub mod server;
pub mod web;
pub mod window;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::net::httpd;
use crate::net::tunnel::Provider;
use crate::storage::Storage;
use crate::util::stop::Stop;

pub use bundle::Bundle;
pub use hub::Hub;
pub use window::WindowMode;

/// The LAN listener's port when free (`--lan`; the next of the following
/// 20 otherwise), as the old party host had it.
pub const LAN_PORT: u16 = 7420;
/// With a window we do not own: no page connected for this long ends
/// the run.
pub const ORPHAN_AFTER: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(200);

/// Who carries the phones' link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TunnelChoice {
    /// OpenTunnel (its identity is reused), else Cloudflare.
    #[default]
    Auto,
    Provider(Provider),
    /// No public link (`--no-tunnel`).
    Off,
}

/// `tvpipe playshare` (the CLI's options).
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The bundle: a folder or a `.zip`.
    pub path: PathBuf,
    pub tunnel: TunnelChoice,
    /// `--lan [IP]`: `Some(None)` for the default route's address.
    pub lan: Option<Option<IpAddr>>,
    pub window: WindowMode,
    /// `--browser`: the window's and the TV's browser.
    pub browser: Option<PathBuf>,
    /// The local listener's port (0: any).
    pub port: u16,
}

/// A started playshare: the hub, its listeners and the tunnel.
pub(crate) struct Running {
    pub hub: Arc<Hub>,
    servers: Servers,
    tunnel: TunnelThread,
}

impl Running {
    /// Binds the listeners and starts the tunnel (in the background).
    pub(crate) fn start(
        bundle: Bundle,
        backend: Box<dyn backend::Backend>,
        opts: &Options,
        stop: Stop,
    ) -> Result<Running> {
        let token = crate::util::hex::encode(&crate::crypto::ring::random_array::<16>()?);
        let hub = Hub::new(bundle, token, backend, stop);
        let servers = Servers::start(&hub, opts.port, opts.lan)?;
        let tunnel = start_tunnel(&hub, opts.tunnel);
        Ok(Running {
            hub,
            servers,
            tunnel,
        })
    }

    /// The window's first URL (it sets the session cookie).
    pub(crate) fn start_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/_tvpipe/start?t={}",
            self.hub.ports.get().map(|p| p.local).unwrap_or(0),
            self.hub.token
        )
    }

    /// The end (§10).
    pub(crate) fn end(self) {
        self.hub.shutdown();
        drop(self.tunnel);
        drop(self.servers);
    }
}

/// Runs playshare until the window closes, the page quits or `stop` is
/// requested. Prints the window's URL with `--window none`.
pub fn run(opts: &Options, stop: Stop, storage: Storage) -> Result<()> {
    let bundle = Bundle::open(&opts.path)?;
    info!(
        "playshare: {} from {}",
        bundle.manifest.title,
        bundle.origin.display()
    );
    let backend = backend::Real {
        storage,
        browser: opts.browser.clone(),
    };
    let running = Running::start(bundle, Box::new(backend), opts, stop.clone())?;
    let hub = running.hub.clone();
    let p = *hub.ports.get().ok_or_else(|| Error::closed("no ports"))?;
    info!(
        "playshare: phones on this computer: http://127.0.0.1:{}/{}",
        p.public,
        if let Some(lan) = p.lan {
            format!(", on the Wi-Fi: http://{lan}/")
        } else {
            String::new()
        }
    );
    let url = running.start_url();
    let (mut window, mode) = match window::Window::open(opts.window, &url, opts.browser.as_ref()) {
        Ok(w) => w,
        Err(e) => {
            running.end();
            return Err(e);
        }
    };
    if mode == WindowMode::None {
        println!("{url}");
        info!("playshare: open {url}");
    }
    let why = loop {
        if stop.wait(TICK) {
            break "stopped";
        }
        hub.tick();
        if window.owned() {
            if window.closed() {
                break "the window was closed";
            }
        } else if hub.apis_gone_for().is_some_and(|d| d > ORPHAN_AFTER) {
            break "no page has been connected for 30 s";
        }
    };
    info!("playshare: ending ({why})");
    // The pages first (goodbyes to phones), then the window.
    running.end();
    window.close();
    Ok(())
}

/// The listeners.
struct Servers {
    list: Vec<httpd::Server>,
}

impl Servers {
    fn start(hub: &Arc<Hub>, port: u16, lan: Option<Option<IpAddr>>) -> Result<Servers> {
        let config = httpd::Config {
            max_connections: hub.max_clients() + 64,
            max_body: 64 * 1024,
            lan: false,
        };
        let local = httpd::Server::bind_with(
            SocketAddr::from(([127, 0, 0, 1], port)),
            server::handler(hub.clone(), server::Listener::Local),
            config,
        )?;
        let public = httpd::Server::bind_with(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            server::handler(hub.clone(), server::Listener::Public),
            config,
        )?;
        let mut list = vec![];
        let mut lan_addr = None;
        if let Some(ip) = lan {
            let ip = match ip {
                Some(ip) => ip,
                None => IpAddr::V4(crate::net::iface::default_ipv4()?),
            };
            let cfg = httpd::Config {
                lan: true,
                ..config
            };
            let mut bound = None;
            let mut last = None;
            for port in (LAN_PORT..LAN_PORT + 20).chain([0]) {
                match httpd::Server::bind_with(
                    SocketAddr::new(ip, port),
                    server::handler(hub.clone(), server::Listener::Lan),
                    cfg,
                ) {
                    Ok(s) => {
                        bound = Some(s);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            let s = match bound {
                Some(s) => s,
                None => {
                    return Err(last.unwrap_or_else(|| Error::closed("no LAN port")));
                }
            };
            lan_addr = Some(s.local_addr());
            list.push(s);
        }
        let _ = hub.ports.set(hub::Ports {
            local: local.local_addr().port(),
            public: public.local_addr().port(),
            lan: lan_addr,
        });
        list.push(local);
        list.push(public);
        Ok(Servers { list })
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        for s in &mut self.list {
            s.shutdown();
        }
    }
}

/// A running tunnel, dropped at the end.
struct TunnelThread {
    slot: Arc<std::sync::Mutex<Option<Box<dyn crate::net::tunnel::Tunnel>>>>,
}

impl Drop for TunnelThread {
    fn drop(&mut self) {
        let t = self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(t);
    }
}

/// Starts the tunnel in the background (§11): the API reports it when it
/// is up.
fn start_tunnel(hub: &Arc<Hub>, choice: TunnelChoice) -> TunnelThread {
    let slot: Arc<std::sync::Mutex<Option<Box<dyn crate::net::tunnel::Tunnel>>>> = Arc::default();
    let order: Vec<Provider> = match choice {
        TunnelChoice::Off => return TunnelThread { slot },
        TunnelChoice::Auto => vec![Provider::OpenTunnel, Provider::Cloudflare],
        TunnelChoice::Provider(p) => vec![p],
    };
    let port = hub.ports.get().map(|p| p.public).unwrap_or(0);
    {
        let mut t = hub.tunnel.write().unwrap_or_else(|e| e.into_inner());
        t.state = "starting";
    }
    let (hub, s) = (hub.clone(), slot.clone());
    let _ = std::thread::Builder::new()
        .name("playshare-tunnel".into())
        .spawn(move || {
            let stop = hub.stop.clone();
            let mut last_err = None;
            for p in order {
                if stop.requested() {
                    return;
                }
                match crate::net::tunnel::start(port, p, &|| stop.requested()) {
                    Ok(t) => {
                        info!(
                            "playshare: phones join at https://{}/ ({})",
                            t.host(),
                            t.name()
                        );
                        {
                            let mut info = hub.tunnel.write().unwrap_or_else(|e| e.into_inner());
                            info.state = "up";
                            info.provider = Some(p.name().to_owned());
                            info.host = Some(t.host().to_owned());
                            info.error = None;
                        }
                        *hub.peers.write().unwrap_or_else(|e| e.into_inner()) = t.peer_lookup();
                        *s.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
                        if stop.requested() {
                            s.lock().unwrap_or_else(|e| e.into_inner()).take();
                            return;
                        }
                        hub.tunnel_changed();
                        return;
                    }
                    Err(e) => {
                        warn!("playshare: {} tunnel: {e}", p.name());
                        last_err = Some(e.to_string());
                    }
                }
            }
            {
                let mut info = hub.tunnel.write().unwrap_or_else(|e| e.into_inner());
                info.state = "failed";
                info.error = last_err;
            }
            hub.tunnel_changed();
        });
    TunnelThread { slot }
}

#[cfg(test)]
mod tests;
