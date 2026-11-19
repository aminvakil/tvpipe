//! Short descriptions of visitors for people at the terminal: the browser
//! and system behind a user agent ("Safari 18 on iPhone"), the address a
//! tunnel reports and Cloudflare's country. All of it comes from the
//! network, so what comes out is plain printable ASCII, cut short.

/// The browser in a user agent: "Chrome 131", "Safari 18", "Firefox 133",
/// "Edge 131"; empty when it is none of the common ones.
pub fn browser_name(agent: &str) -> String {
    // Most specific first: Edge and the iOS browsers also say Chrome or
    // Safari, Chrome also says Safari.
    const TOKENS: [(&str, &str); 7] = [
        ("Firefox", "Firefox"),
        ("FxiOS", "Firefox"),
        ("EdgiOS", "Edge"),
        ("Edg", "Edge"),
        ("SamsungBrowser", "Samsung Internet"),
        ("CriOS", "Chrome"),
        ("Chrome", "Chrome"),
    ];
    let find = |token: &str, label: &str| {
        agent.split_whitespace().find_map(|t| {
            let v = t.strip_prefix(token)?.strip_prefix('/')?;
            Some(format!("{label} {}", major(v)?))
        })
    };
    TOKENS
        .iter()
        .find_map(|(t, l)| find(t, l))
        .or_else(|| find("Version", "Safari").filter(|_| agent.contains("Safari")))
        .unwrap_or_default()
}

/// The system in a user agent: "iPhone", "iPad", "Android", "macOS",
/// "Windows", "ChromeOS", "Linux"; `None` when it says none of them.
pub fn system_name(agent: &str) -> Option<&'static str> {
    // iPads ask for desktop pages and say Macintosh, so iPhone and iPad
    // come first and a Mac is only a Mac without them.
    const SYSTEMS: [(&str, &str); 8] = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("CrOS", "ChromeOS"),
        ("Macintosh", "macOS"),
        ("Mac OS X", "macOS"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ];
    SYSTEMS
        .iter()
        .find(|(t, _)| agent.contains(t))
        .map(|(_, name)| *name)
}

/// "Safari 18 on iPhone", "Chrome 131", "a browser on Android"; empty when
/// the user agent says neither.
pub fn describe(agent: &str) -> String {
    match (browser_name(agent), system_name(agent)) {
        (b, Some(s)) if !b.is_empty() => format!("{b} on {s}"),
        (b, None) => b,
        (_, Some(s)) => format!("a browser on {s}"),
    }
}

/// The address in what a tunnel reports ("203.0.113.9:4000", "[2001:db8::1]:4000",
/// "203.0.113.9"), as an address and nothing else.
pub fn address(peer: &str) -> Option<String> {
    if let Ok(a) = peer.parse::<std::net::SocketAddr>() {
        return Some(a.ip().to_string());
    }
    peer.parse::<std::net::IpAddr>().ok().map(|a| a.to_string())
}

/// Cloudflare's `cf-ipcountry`: two letters, uppercase. Not the codes it
/// uses for unknown places (`XX`) and Tor (`T1`).
pub fn country(header: &str) -> Option<String> {
    let c = header.trim();
    (c.len() == 2 && c.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| c.to_ascii_uppercase())
        .filter(|c| c != "XX")
}

/// The leading digits of a version ("131.0.0.0" → "131").
fn major(version: &str) -> Option<&str> {
    let end = version
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(version.len());
    (end > 0).then(|| &version[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    const SAFARI_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
    const FIREFOX_MAC: &str =
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:133.0) Gecko/20100101 Firefox/133.0";
    const EDGE_WIN: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
    const SAFARI_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
    const CHROME_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/131.0.6778.73 Mobile/15E148 Safari/604.1";
    const CHROME_ANDROID: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";
    const SAMSUNG: &str = "Mozilla/5.0 (Linux; Android 14; SM-S918B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0.0.0 Mobile Safari/537.36";

    #[test]
    fn browsers() {
        assert_eq!(browser_name(CHROME_MAC), "Chrome 131");
        assert_eq!(browser_name(SAFARI_MAC), "Safari 18");
        assert_eq!(browser_name(FIREFOX_MAC), "Firefox 133");
        assert_eq!(browser_name(EDGE_WIN), "Edge 131");
        assert_eq!(browser_name(SAFARI_IPHONE), "Safari 18");
        assert_eq!(browser_name(CHROME_IPHONE), "Chrome 131");
        assert_eq!(browser_name(SAMSUNG), "Samsung Internet 26");
        assert_eq!(browser_name("curl/8"), "");
        assert_eq!(browser_name(""), "");
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe(CHROME_MAC), "Chrome 131 on macOS");
        assert_eq!(describe(EDGE_WIN), "Edge 131 on Windows");
        assert_eq!(describe(SAFARI_IPHONE), "Safari 18 on iPhone");
        assert_eq!(describe(CHROME_IPHONE), "Chrome 131 on iPhone");
        assert_eq!(describe(CHROME_ANDROID), "Chrome 131 on Android");
        assert_eq!(
            describe("Mozilla/5.0 (Linux; Android 14) Unknown/1"),
            "a browser on Android"
        );
        assert_eq!(
            describe("Mozilla/5.0 Chrome/131.0.0.0 Safari/537.36"),
            "Chrome 131"
        );
        assert_eq!(describe("curl/8"), "");
    }

    #[test]
    fn nothing_from_the_network_reaches_the_terminal_unchecked() {
        // A version is digits only, whatever follows it.
        assert_eq!(browser_name("Chrome/131\x1b[2J"), "Chrome 131");
        assert_eq!(browser_name("Chrome/\x1b[2J"), "");
        assert_eq!(address("203.0.113.9:4000").as_deref(), Some("203.0.113.9"));
        assert_eq!(
            address("[2001:db8::1]:4000").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(address("198.51.100.7").as_deref(), Some("198.51.100.7"));
        assert_eq!(address("evil\x1b[2J"), None);
        assert_eq!(address(""), None);
        assert_eq!(country("de").as_deref(), Some("DE"));
        assert_eq!(country(" CH ").as_deref(), Some("CH"));
        assert_eq!(country("XX"), None);
        assert_eq!(country("T1"), None);
        assert_eq!(country("\x1b["), None);
        assert_eq!(country("DEU"), None);
    }
}
