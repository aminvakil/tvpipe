//! `tvpipe discover` and `--to NAME` resolution (PLAN §4, §6).

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use super::device::{self, Device};
use super::iface;
use super::mdns::{self, BrowseOptions, Cache};
use crate::airplay::credentials::Store;
use crate::airplay::discovery::format_features;
use crate::codec::json::Value;
use crate::error::Result;
use crate::util::hex;

/// Browses for `timeout` and returns the merged devices.
pub fn discover(timeout: Duration, interfaces: &[Ipv4Addr]) -> Result<Vec<Device>> {
    let cache = browse(timeout, interfaces, |_| false)?;
    Ok(device::devices(&cache))
}

/// Resolves `--to QUERY`. An address is returned without browsing as a
/// bare device-less hint (`Ok(None)`); otherwise browses until a device
/// with exactly that name shows up (or the timeout passes), then applies
/// [`device::select`].
pub fn resolve(query: &str, timeout: Duration, interfaces: &[Ipv4Addr]) -> Result<Option<Device>> {
    if query.parse::<IpAddr>().is_ok() {
        return Ok(None);
    }
    let q = query.to_lowercase();
    let cache = browse(timeout, interfaces, |cache| {
        device::devices(cache)
            .iter()
            .any(|d| d.names().iter().any(|n| n.to_lowercase() == q))
    })?;
    let devices = device::devices(&cache);
    device::select(&devices, query).cloned().map(Some)
}

fn browse(
    timeout: Duration,
    interfaces: &[Ipv4Addr],
    done: impl FnMut(&Cache) -> bool,
) -> Result<Cache> {
    let opts = BrowseOptions {
        services: device::services(),
        interfaces: iface::browse_addrs(interfaces),
        timeout,
        target: BrowseOptions::MDNS,
    };
    mdns::browse(&opts, done)
}

/// The `discover` command body.
pub fn run(timeout: Duration, json: bool, interfaces: &[Ipv4Addr]) -> Result<()> {
    let devices = discover(timeout, interfaces)?;
    let store = stored_pairings();
    if crate::machine::enabled() {
        let list = Value::array(devices.iter().map(|d| device_json(d, store.as_ref())));
        crate::machine::emit(crate::machine::event("devices").with("devices", list));
        return Ok(());
    }
    let out = if json {
        to_json_with(&devices, store.as_ref())
    } else {
        to_table(&devices)
    };
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(out.as_bytes())
        .and_then(|()| stdout.flush())
    {
        // A closed pipe (`| head`) is not an error worth reporting.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        r => r.map_err(|e| crate::error::Error::io("write discover output", e)),
    }
}

fn protocols(d: &Device) -> String {
    let mut p = Vec::new();
    if d.airplay.is_some() {
        p.push(if d.airplay_mirroring() {
            "airplay(mirror)"
        } else {
            "airplay"
        });
    }
    if d.raop.is_some() {
        p.push("raop");
    }
    if d.cast.is_some() {
        p.push(if d.cast_video() {
            "cast(video)"
        } else {
            "cast"
        });
    }
    p.join(",")
}

/// A plain table, one device per line.
pub fn to_table(devices: &[Device]) -> String {
    if devices.is_empty() {
        return "no receivers found\n".to_owned();
    }
    let rows: Vec<[String; 4]> = devices
        .iter()
        .map(|d| {
            [
                d.name.clone(),
                d.addrs().first().map(IpAddr::to_string).unwrap_or_default(),
                protocols(d),
                d.model().unwrap_or("").to_owned(),
            ]
        })
        .collect();
    let header = ["NAME", "ADDRESS", "PROTOCOLS", "MODEL"].map(str::to_owned);
    let mut widths = [0usize; 4];
    for row in std::iter::once(&header).chain(&rows) {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in std::iter::once(&header).chain(&rows) {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if i + 1 < row.len() {
                let pad = widths[i] - cell.chars().count() + 2;
                line.extend(std::iter::repeat_n(' ', pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Machine-readable output: a JSON array of devices.
pub fn to_json(devices: &[Device]) -> String {
    to_json_with(devices, None)
}

/// [`to_json`] with `paired` on AirPlay entries when the store is given.
fn to_json_with(devices: &[Device], store: Option<&Store>) -> String {
    let list = Value::array(devices.iter().map(|d| device_json(d, store)));
    format!("{list:#}\n")
}

/// The AirPlay pairings, if the credentials file exists (discovery never
/// creates it).
fn stored_pairings() -> Option<Store> {
    let path = crate::airplay::credentials::default_path().ok()?;
    let text = std::fs::read_to_string(&path).ok()?;
    Store::parse(&path, &text)
        .map_err(|e| debug!("discover: {e}"))
        .ok()
}

/// Whether `store` has a pairing for `d` (by key, device id or name).
fn is_paired(d: &Device, store: &Store) -> bool {
    let txt = d.airplay.as_ref().or(d.raop.as_ref()).map(|a| &a.txt);
    let q = crate::airplay::credentials::Query {
        public_key: txt.and_then(|t| t.pk.as_deref()),
        device_id: txt.and_then(|t| t.deviceid.as_deref()),
        names: d.names(),
        addr: None,
    };
    store.find(&q).is_some()
}

fn device_json(d: &Device, store: Option<&Store>) -> Value {
    let mut v = Value::object()
        .with("name", d.name.as_str())
        .with(
            "addresses",
            Value::array(d.addrs().iter().map(|a| a.to_string())),
        )
        .with_opt("model", d.model());
    for (key, ap) in [("airplay", &d.airplay), ("raop", &d.raop)] {
        let Some(ap) = ap else { continue };
        let t = &ap.txt;
        let o = endpoint_json(&ap.endpoint)
            .with_opt("model", t.model.as_deref())
            .with_opt("srcvers", t.srcvers.as_deref())
            .with_opt("osvers", t.osvers.as_deref())
            .with_opt("deviceid", t.deviceid.as_deref())
            .with_opt("features", t.features.map(format_features))
            .with_opt("flags", t.flags.map(|f| format!("{f:#x}")))
            .with_opt("pk", t.pk.as_deref().map(hex::encode))
            .with("mirroring", t.supports_mirroring())
            .with("password", t.password_required())
            .with("pin", t.pin_required())
            .with_opt("paired", store.map(|s| is_paired(d, s)));
        v = v.with(key, o);
    }
    if let Some(c) = &d.cast {
        let t = &c.txt;
        let o = endpoint_json(&c.endpoint)
            .with_opt("friendly_name", t.friendly_name.as_deref())
            .with_opt("model", t.model.as_deref())
            .with_opt("id", t.id.as_deref())
            .with_opt("capabilities", t.capabilities)
            .with_opt("status", t.status.as_deref())
            .with("video", d.cast_video())
            .with("group", t.is_group());
        v = v.with("cast", o);
    }
    v
}

fn endpoint_json(ep: &device::Endpoint) -> Value {
    Value::object()
        .with("instance", ep.instance.as_str())
        .with("host", ep.host.as_str())
        .with("port", ep.port)
        .with("address", ep.addr().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::json;

    #[test]
    fn empty_outputs() {
        assert_eq!(to_json(&[]), "[]\n");
        assert_eq!(to_table(&[]), "no receivers found\n");
    }

    #[test]
    fn lan_capture_json() {
        let devices = crate::net::device::tests::lan_devices();
        let v = json::parse(&to_json(&devices)).unwrap();
        let list = v.as_array().unwrap();
        assert_eq!(list.len(), 13);
        let tv = list
            .iter()
            .find(|d| d["name"].as_str() == Some("Livingroom (2)"))
            .unwrap();
        assert_eq!(tv["model"].as_str(), Some("AppleTV6,2"));
        assert_eq!(tv["addresses"][0].as_str(), Some("192.168.1.34"));
        let ap = &tv["airplay"];
        assert_eq!(ap["port"].as_u64(), Some(7000));
        assert_eq!(ap["deviceid"].as_str(), Some("AA:BB:CC:00:00:0A"));
        assert_eq!(ap["features"].as_str(), Some("0x4A7FDFD5,0x3C177FDE"));
        assert_eq!(ap["flags"].as_str(), Some("0x644"));
        assert_eq!(ap["mirroring"].as_bool(), Some(true));
        assert_eq!(ap["pk"].as_str().map(str::len), Some(64));
        assert!(tv["raop"]["instance"]
            .as_str()
            .unwrap()
            .ends_with("@Livingroom"));
        assert!(tv["cast"].is_null());
    }
}
