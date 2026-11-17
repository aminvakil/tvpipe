//! `streamlib discover` and `--to NAME` resolution (PLAN §4, §6).

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use super::device::{self, Device};
use super::iface;
use super::mdns::{self, BrowseOptions, Cache};
use crate::airplay::discovery::format_features;
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
    let out = if json {
        to_json(&devices)
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
    let mut out = String::from("[");
    for (i, d) in devices.iter().enumerate() {
        out.push_str(if i == 0 { "\n  {" } else { ",\n  {" });
        let mut f = Fields::new(&mut out, 4);
        f.str("name", &d.name);
        f.raw(
            "addresses",
            &format!(
                "[{}]",
                d.addrs()
                    .iter()
                    .map(|a| json_str(&a.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        f.opt_str("model", d.model());
        for (key, ap) in [("airplay", &d.airplay), ("raop", &d.raop)] {
            let Some(ap) = ap else { continue };
            let mut o = String::from("{");
            let mut g = Fields::new(&mut o, 6);
            endpoint_fields(&mut g, &ap.endpoint);
            let t = &ap.txt;
            g.opt_str("model", t.model.as_deref());
            g.opt_str("srcvers", t.srcvers.as_deref());
            g.opt_str("osvers", t.osvers.as_deref());
            g.opt_str("deviceid", t.deviceid.as_deref());
            g.opt_str("features", t.features.map(format_features).as_deref());
            g.opt_str("flags", t.flags.map(|f| format!("{f:#x}")).as_deref());
            g.opt_str("pk", t.pk.as_deref().map(hex::encode).as_deref());
            g.raw("mirroring", bool_str(t.supports_mirroring()));
            g.raw("password", bool_str(t.password_required()));
            g.raw("pin", bool_str(t.pin_required()));
            g.finish();
            f.raw(key, &o);
        }
        if let Some(c) = &d.cast {
            let mut o = String::from("{");
            let mut g = Fields::new(&mut o, 6);
            endpoint_fields(&mut g, &c.endpoint);
            let t = &c.txt;
            g.opt_str("friendly_name", t.friendly_name.as_deref());
            g.opt_str("model", t.model.as_deref());
            g.opt_str("id", t.id.as_deref());
            if let Some(ca) = t.capabilities {
                g.raw("capabilities", &ca.to_string());
            }
            g.opt_str("status", t.status.as_deref());
            g.raw("video", bool_str(d.cast_video()));
            g.raw("group", bool_str(t.is_group()));
            g.finish();
            f.raw("cast", &o);
        }
        f.finish();
    }
    out.push_str(if devices.is_empty() { "]\n" } else { "\n]\n" });
    out
}

fn endpoint_fields(g: &mut Fields<'_>, ep: &device::Endpoint) {
    g.str("instance", &ep.instance);
    g.str("host", &ep.host);
    g.raw("port", &ep.port.to_string());
    g.str("address", &ep.addr().to_string());
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

/// Writes `"key": value` members of one JSON object, one per line.
struct Fields<'a> {
    out: &'a mut String,
    indent: usize,
    first: bool,
}

impl<'a> Fields<'a> {
    fn new(out: &'a mut String, indent: usize) -> Fields<'a> {
        Fields {
            out,
            indent,
            first: true,
        }
    }

    fn raw(&mut self, key: &str, value: &str) {
        let sep = if self.first { "" } else { "," };
        self.first = false;
        let _ = write!(
            self.out,
            "{sep}\n{:indent$}{}: {value}",
            "",
            json_str(key),
            indent = self.indent
        );
    }

    fn str(&mut self, key: &str, value: &str) {
        self.raw(key, &json_str(value));
    }

    fn opt_str(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.str(key, v);
        }
    }

    fn finish(self) {
        let _ = write!(self.out, "\n{:indent$}}}", "", indent = self.indent - 2);
    }
}

/// A JSON string literal (RFC 8259 §7).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes() {
        assert_eq!(json_str("a\"b\\c\n\u{1}é"), "\"a\\\"b\\\\c\\n\\u0001é\"");
    }

    #[test]
    fn empty_outputs() {
        assert_eq!(to_json(&[]), "[]\n");
        assert_eq!(to_table(&[]), "no receivers found\n");
    }
}
