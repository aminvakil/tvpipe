//! What the API drives: discovery, pairing and the cast. The real one uses
//! the public API ([`Discovery`](crate::api::Discovery), `Pairing`,
//! [`share`](crate::api::share)); tests put fakes in its place.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::codec::json::Value;
use crate::error::Result;
use crate::storage::Storage;

/// Discovery while the launcher watches.
pub trait Discover: Send {
    /// The receivers found so far (the API's device objects).
    fn devices(&self) -> Vec<Value>;
}

/// One pairing.
pub trait Pair: Send {
    /// `{state: connecting|pin|done|failed, error?}`.
    fn state(&self) -> Value;
    fn submit(&self, pin: &str) -> Result<()>;
    fn cancel(&self);
}

/// One cast session.
pub trait Cast: Send {
    /// `{state: starting|streaming|failed|stopped, receivers, player?, error?}`.
    fn status(&mut self) -> Value;
    /// TEARDOWN to the receivers, the headless browser quits.
    fn stop(self: Box<Self>);
}

pub trait Backend: Send + Sync {
    fn discover(&self) -> Result<Box<dyn Discover>>;
    fn pair(&self, device: &str) -> Result<Box<dyn Pair>>;
    /// Casts the page at `url` (on the local origin) to `to`.
    fn cast(&self, url: &str, to: &[String], size: Option<(u32, u32)>) -> Result<Box<dyn Cast>>;
}

/// The API's view of a receiver.
pub fn device_value(d: &crate::net::discover::DeviceInfo) -> Option<Value> {
    let airplay = d.airplay.as_ref().filter(|a| a.mirroring);
    let cast = d.cast.as_ref().filter(|c| c.video && !c.group);
    let kind = match (airplay.is_some(), cast.is_some()) {
        (true, true) => "both",
        (true, false) => "airplay",
        (false, true) => "cast",
        (false, false) => return None,
    };
    let paired = airplay.and_then(|a| a.paired);
    // A receiver that shows a PIN and has no pairing here needs one
    // (cast picks Cast for "both", so only AirPlay-only ones).
    let needs_pin = kind == "airplay" && airplay.is_some_and(|a| a.pin) && paired != Some(true);
    Some(
        Value::object()
            .with("name", d.name.as_str())
            .with("kind", kind)
            .with_opt("model", d.model.as_deref())
            .with("needsPairing", needs_pin)
            .with_opt("paired", paired),
    )
}

/// The real thing.
pub struct Real {
    pub storage: Storage,
    pub browser: Option<PathBuf>,
}

struct RealDiscover(crate::api::Discovery);

impl Discover for RealDiscover {
    fn devices(&self) -> Vec<Value> {
        self.0.devices().iter().filter_map(device_value).collect()
    }
}

#[cfg(feature = "airplay")]
struct RealPair(crate::api::Pairing);

#[cfg(feature = "airplay")]
impl Pair for RealPair {
    fn state(&self) -> Value {
        use crate::api::PairingState;
        match self.0.state() {
            PairingState::Connecting => Value::object().with("state", "connecting"),
            PairingState::NeedPin(_) => Value::object().with("state", "pin"),
            PairingState::Done(_) => Value::object().with("state", "done"),
            PairingState::Failed(e) => Value::object()
                .with("state", "failed")
                .with("error", e.message()),
        }
    }

    fn submit(&self, pin: &str) -> Result<()> {
        self.0.submit_pin(pin)
    }

    fn cancel(&self) {
        self.0.cancel();
    }
}

struct RealCast {
    session: Mutex<crate::api::Session>,
    error: Option<String>,
}

impl Cast for RealCast {
    fn status(&mut self) -> Value {
        use crate::api::SessionState;
        let s = self.session.get_mut().unwrap_or_else(|e| e.into_inner());
        for e in s.poll_events() {
            if let crate::events::Event::Error(info) = e {
                self.error = Some(info.message.clone());
            }
        }
        let state = match s.state() {
            SessionState::Connecting => "starting",
            SessionState::Streaming => "streaming",
            _ if self.error.is_some() => "failed",
            _ => "stopped",
        };
        let receivers: Vec<Value> = s
            .receivers()
            .iter()
            .map(|r| {
                Value::object()
                    .with("name", r.name.as_str())
                    .with_opt("protocol", r.protocol.map(|p| p.as_str()))
                    .with("state", r.state.as_str())
                    .with_opt("error", r.error.as_deref())
                    .with_opt("code", r.code.as_deref())
            })
            .collect();
        Value::object()
            .with("state", state)
            .with("receivers", Value::array(receivers))
            .with_opt("player", s.player_url())
            .with_opt("error", self.error.as_deref())
    }

    fn stop(self: Box<Self>) {
        let s = self.session.into_inner().unwrap_or_else(|e| e.into_inner());
        let _ = s.stop();
    }
}

impl Backend for Real {
    fn discover(&self) -> Result<Box<dyn Discover>> {
        let opts = crate::api::Options::new().storage(self.storage.clone());
        Ok(Box::new(RealDiscover(crate::api::Discovery::start(&opts)?)))
    }

    #[cfg(feature = "airplay")]
    fn pair(&self, device: &str) -> Result<Box<dyn Pair>> {
        let opts = crate::api::Options::new().storage(self.storage.clone());
        Ok(Box::new(RealPair(crate::api::Pairing::start(
            device, &opts,
        )?)))
    }

    #[cfg(not(feature = "airplay"))]
    fn pair(&self, _device: &str) -> Result<Box<dyn Pair>> {
        Err(crate::error::Error::unsupported("pairing needs AirPlay"))
    }

    fn cast(&self, url: &str, to: &[String], size: Option<(u32, u32)>) -> Result<Box<dyn Cast>> {
        let mut opts = crate::api::Options::new().storage(self.storage.clone());
        for t in to {
            opts = opts.to(t.as_str());
        }
        if let Some((w, h)) = size {
            opts = opts.size(w, h);
        }
        if let Some(b) = &self.browser {
            opts = opts.browser_path(b.clone());
        }
        let session = crate::api::share(Some(url), opts)?;
        Ok(Box::new(RealCast {
            session: Mutex::new(session),
            error: None,
        }))
    }
}
