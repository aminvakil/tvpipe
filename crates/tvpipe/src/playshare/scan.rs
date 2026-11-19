//! What a game's files load (`tvpipe bundle`'s checks): the references
//! in HTML (`src`, `href`, `srcset`, `poster`, inline scripts and
//! styles, import maps), in CSS (`url()`, `@import`) and in JavaScript
//! modules (static `import`/`export … from`, `import("…")` with a
//! literal). Tolerant scanners, not parsers: they never fail, and what
//! they cannot see (a URL built at run time) they do not report.
//!
//! Sources: the WHATWG HTML tokenizer's shape (§13.2.5: tags, attributes,
//! comments, raw text in `script` and `style`), CSS Syntax 3 (`url()`,
//! strings, comments), ECMA-262 §12 (tokens; a `/` starts a regular
//! expression where an expression may start) and §16.2.2 (import and
//! export declarations).

/// What kind of file, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Html,
    Css,
    Js,
    Other,
}

pub fn kind_of(path: &str) -> Kind {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => Kind::Html,
        "css" => Kind::Css,
        "js" | "mjs" => Kind::Js,
        _ => Kind::Other,
    }
}

/// How a reference loads its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// `<script src>`: `module` for `type="module"`.
    Script { module: bool },
    /// `<link rel="modulepreload">`.
    ModulePreload,
    /// `<link rel="stylesheet">`, CSS `@import`.
    Stylesheet,
    /// `<iframe src>`, `<frame src>`: a page.
    Frame,
    /// A link a person follows (`<a href>`, `<area href>`, `<form action>`).
    Link,
    /// Everything else a page or style sheet loads (images, fonts, media,
    /// icons, the web app manifest).
    Asset,
    /// A JS module's static import or re-export.
    Import,
    /// `import("…")` with a literal.
    DynamicImport,
}

/// One reference: the URL as written, its line (1-based), how it loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub url: String,
    pub line: u32,
    pub kind: RefKind,
}

/// An inline script or style sheet (its text and first line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub line: u32,
    /// Scripts: `type="module"`.
    pub module: bool,
}

/// What an HTML document loads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Html {
    pub refs: Vec<Ref>,
    pub scripts: Vec<Inline>,
    pub styles: Vec<Inline>,
    /// `<script type="importmap">` contents.
    pub import_maps: Vec<Inline>,
    /// `<base href>`: resolving changes (reported, not followed).
    pub base: Option<(String, u32)>,
}

/// Line numbers from byte offsets.
struct Lines(Vec<usize>);

impl Lines {
    fn new(s: &str) -> Lines {
        Lines(
            s.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i)
                .collect(),
        )
    }

    fn at(&self, offset: usize) -> u32 {
        (self.0.partition_point(|&nl| nl < offset) + 1) as u32
    }
}

fn find_ci(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len())
        .find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Decodes the character references attribute values use most.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let end = rest[..rest.len().min(12)].find(';');
        let decoded = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => {
                    let n = if let Some(h) = name.strip_prefix("#x").or(name.strip_prefix("#X")) {
                        u32::from_str_radix(h, 16).ok()
                    } else if let Some(d) = name.strip_prefix('#') {
                        d.parse().ok()
                    } else {
                        None
                    };
                    n.and_then(char::from_u32)
                }
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One start tag's attributes (names lowercased, values decoded).
fn attributes(b: &[u8], mut i: usize) -> (Vec<(String, String, usize)>, usize) {
    let mut attrs = Vec::new();
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            return (attrs, i);
        }
        if b[i] == b'>' {
            return (attrs, i + 1);
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = String::from_utf8_lossy(&b[start..i]).to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        let at = i;
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let s = i + 1;
                i = s;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                value = String::from_utf8_lossy(&b[s..i]).into_owned();
                i = (i + 1).min(b.len());
            } else {
                let s = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                value = String::from_utf8_lossy(&b[s..i]).into_owned();
            }
        }
        if !name.is_empty() {
            attrs.push((name, decode_entities(&value), at));
        } else {
            i += 1;
        }
    }
}

/// Scans an HTML document.
pub fn html(src: &str) -> Html {
    let lines = Lines::new(src);
    let b = src.as_bytes();
    let mut out = Html::default();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"<!--") {
            i = find_ci(b, i + 4, b"-->").map_or(b.len(), |e| e + 3);
            continue;
        }
        if i + 1 < b.len() && matches!(b[i + 1], b'!' | b'?' | b'/') {
            i = b[i..]
                .iter()
                .position(|&c| c == b'>')
                .map_or(b.len(), |e| i + e + 1);
            continue;
        }
        if !(i + 1 < b.len() && b[i + 1].is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        let ns = i + 1;
        let mut j = ns;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
            j += 1;
        }
        let tag = String::from_utf8_lossy(&b[ns..j]).to_ascii_lowercase();
        let (attrs, end) = attributes(b, j);
        let get = |n: &str| attrs.iter().find(|a| a.0 == n);
        let mut push = |url: &str, at: usize, kind: RefKind| {
            out.refs.push(Ref {
                url: url.trim().to_owned(),
                line: lines.at(at),
                kind,
            });
        };
        match tag.as_str() {
            "script" => {
                let ty = get("type").map(|a| a.1.trim().to_ascii_lowercase());
                let module = ty.as_deref() == Some("module");
                if let Some((_, v, at)) = get("src") {
                    push(v, *at, RefKind::Script { module });
                }
            }
            "link" => {
                let rel = get("rel")
                    .map(|a| a.1.to_ascii_lowercase())
                    .unwrap_or_default();
                let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
                if let Some((_, v, at)) = get("href") {
                    let kind = if rels.contains(&"stylesheet") {
                        RefKind::Stylesheet
                    } else if rels.contains(&"modulepreload") {
                        RefKind::ModulePreload
                    } else if rels.iter().any(|r| {
                        matches!(
                            *r,
                            "preconnect" | "dns-prefetch" | "canonical" | "alternate"
                        )
                    }) {
                        RefKind::Link
                    } else {
                        RefKind::Asset
                    };
                    push(v, *at, kind);
                }
                if let Some((_, v, at)) = get("imagesrcset") {
                    for u in srcset(v) {
                        push(&u, *at, RefKind::Asset);
                    }
                }
            }
            "a" | "area" => {
                if let Some((_, v, at)) = get("href") {
                    if get("download").is_none() {
                        push(v, *at, RefKind::Link);
                    } else {
                        push(v, *at, RefKind::Asset);
                    }
                }
            }
            "form" => {
                if let Some((_, v, at)) = get("action") {
                    push(v, *at, RefKind::Link);
                }
            }
            "iframe" | "frame" => {
                if let Some((_, v, at)) = get("src") {
                    push(v, *at, RefKind::Frame);
                }
            }
            "base" => {
                if let Some((_, v, at)) = get("href") {
                    out.base = Some((v.clone(), lines.at(*at)));
                }
            }
            "object" => {
                if let Some((_, v, at)) = get("data") {
                    push(v, *at, RefKind::Asset);
                }
            }
            _ => {
                if let Some((_, v, at)) = get("src") {
                    push(v, *at, RefKind::Asset);
                }
                if let Some((_, v, at)) = get("srcset") {
                    for u in srcset(v) {
                        push(&u, *at, RefKind::Asset);
                    }
                }
                if let Some((_, v, at)) = get("poster") {
                    push(v, *at, RefKind::Asset);
                }
            }
        }
        if let Some((_, v, at)) = get("style") {
            for r in css(v) {
                push(&r.url, *at, r.kind);
            }
        }
        i = end;
        // Raw text: scripts and styles end only at their end tag.
        if tag == "script" || tag == "style" {
            let close = if tag == "script" {
                &b"</script"[..]
            } else {
                &b"</style"[..]
            };
            let stop = find_ci(b, i, close).unwrap_or(b.len());
            let text = String::from_utf8_lossy(&b[i..stop]).into_owned();
            let line = lines.at(i);
            if tag == "style" {
                out.styles.push(Inline {
                    text,
                    line,
                    module: false,
                });
            } else if get("src").is_none() {
                let ty = get("type").map(|a| a.1.trim().to_ascii_lowercase());
                match ty.as_deref() {
                    Some("importmap") => out.import_maps.push(Inline {
                        text,
                        line,
                        module: false,
                    }),
                    None | Some("") | Some("module") | Some("text/javascript") => {
                        out.scripts.push(Inline {
                            text,
                            line,
                            module: ty.as_deref() == Some("module"),
                        })
                    }
                    _ => {}
                }
            }
            i = stop;
        }
    }
    out
}

/// The URLs of a `srcset` (`url 2x, url 300w`).
fn srcset(v: &str) -> Vec<String> {
    v.split(',')
        .filter_map(|c| c.split_ascii_whitespace().next())
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What a style sheet loads: `url()`s ([`RefKind::Asset`]) and
/// `@import`s ([`RefKind::Stylesheet`]).
pub fn css(src: &str) -> Vec<Ref> {
    let lines = Lines::new(src);
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut import = false;
    let mut i = 0;
    let string = |i: usize| -> (String, usize) {
        let q = b[i];
        let mut j = i + 1;
        let mut s = Vec::new();
        while j < b.len() && b[j] != q && b[j] != b'\n' {
            if b[j] == b'\\' && j + 1 < b.len() {
                j += 1;
            }
            s.push(b[j]);
            j += 1;
        }
        (
            String::from_utf8_lossy(&s).into_owned(),
            (j + 1).min(b.len()),
        )
    };
    while i < b.len() {
        let c = b[i];
        if b[i..].starts_with(b"/*") {
            i = find_ci(b, i + 2, b"*/").map_or(b.len(), |e| e + 2);
        } else if c == b'"' || c == b'\'' {
            let (s, next) = string(i);
            if import {
                out.push(Ref {
                    url: s,
                    line: lines.at(i),
                    kind: RefKind::Stylesheet,
                });
                import = false;
            }
            i = next;
        } else if c == b'@' && b[i..].len() >= 7 && b[i..i + 7].eq_ignore_ascii_case(b"@import") {
            import = true;
            i += 7;
        } else if (c == b'u' || c == b'U')
            && b[i..].len() >= 4
            && b[i..i + 4].eq_ignore_ascii_case(b"url(")
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-'))
        {
            let mut j = i + 4;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let (url, next) = if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let (s, n) = string(j);
                let close = b[n..]
                    .iter()
                    .position(|&c| c == b')')
                    .map_or(b.len(), |p| n + p + 1);
                (s, close)
            } else {
                let e = b[j..]
                    .iter()
                    .position(|&c| c == b')')
                    .map_or(b.len(), |p| j + p);
                (
                    String::from_utf8_lossy(&b[j..e]).trim().to_owned(),
                    (e + 1).min(b.len()),
                )
            };
            out.push(Ref {
                url,
                line: lines.at(i),
                kind: if import {
                    RefKind::Stylesheet
                } else {
                    RefKind::Asset
                },
            });
            import = false;
            i = next;
        } else {
            if c == b';' || c == b'{' {
                import = false;
            }
            i += 1;
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(u8),
    /// A number, a template literal, a regular expression.
    Value,
}

/// The tokens of a script, comments and template texts left out.
fn js_tokens(src: &str) -> Vec<(Tok, u32)> {
    let b = src.as_bytes();
    let mut out: Vec<(Tok, u32)> = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    let mut depth = 0usize;
    // Brace depths where a template's `${` opened.
    let mut templates: Vec<usize> = Vec::new();
    let regex_ok = |out: &Vec<(Tok, u32)>| match out.last().map(|t| &t.0) {
        None => true,
        Some(Tok::Punct(c)) => !matches!(c, b')' | b']'),
        Some(Tok::Ident(w)) => matches!(
            w.as_str(),
            "return"
                | "typeof"
                | "instanceof"
                | "in"
                | "of"
                | "new"
                | "delete"
                | "void"
                | "throw"
                | "case"
                | "do"
                | "else"
                | "yield"
                | "await"
        ),
        Some(_) => false,
    };
    // The rest of a template literal from `i` (after "`" or "}"): to its
    // end ("`", then a value) or to a `${`.
    let template =
        |i: &mut usize, line: &mut u32, templates: &mut Vec<usize>, depth: usize| -> bool {
            while *i < b.len() {
                match b[*i] {
                    b'\\' => *i += 2,
                    b'`' => {
                        *i += 1;
                        return true;
                    }
                    b'$' if b.get(*i + 1) == Some(&b'{') => {
                        *i += 2;
                        templates.push(depth);
                        return false;
                    }
                    c => {
                        if c == b'\n' {
                            *line += 1;
                        }
                        *i += 1;
                    }
                }
            }
            true
        };
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            _ if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = find_ci(b, i + 2, b"*/").map_or(b.len(), |e| e + 2);
                line += b[i..end].iter().filter(|&&x| x == b'\n').count() as u32;
                i = end;
            }
            b'"' | b'\'' => {
                let at = line;
                let mut s = Vec::new();
                i += 1;
                while i < b.len() && b[i] != c && b[i] != b'\n' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        if b[i] == b'\n' {
                            line += 1;
                        }
                    }
                    s.push(b[i]);
                    i += 1;
                }
                i += 1;
                out.push((Tok::Str(String::from_utf8_lossy(&s).into_owned()), at));
            }
            b'`' => {
                let at = line;
                i += 1;
                if template(&mut i, &mut line, &mut templates, depth) {
                    out.push((Tok::Value, at));
                } else {
                    out.push((Tok::Punct(b'('), at));
                }
            }
            b'{' => {
                depth += 1;
                out.push((Tok::Punct(c), line));
                i += 1;
            }
            b'}' => {
                i += 1;
                if templates.last() == Some(&depth) {
                    templates.pop();
                    let at = line;
                    out.push((Tok::Punct(b')'), at));
                    if template(&mut i, &mut line, &mut templates, depth) {
                        out.push((Tok::Value, at));
                    } else {
                        out.push((Tok::Punct(b'('), at));
                    }
                } else {
                    depth = depth.saturating_sub(1);
                    out.push((Tok::Punct(c), line));
                }
            }
            b'/' if regex_ok(&out) => {
                let at = line;
                i += 1;
                let mut class = false;
                while i < b.len() && b[i] != b'\n' {
                    match b[i] {
                        b'\\' => i += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        b'/' if !class => break,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                out.push((Tok::Value, at));
            }
            _ if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 => {
                let s = i;
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric()
                        || b[i] == b'_'
                        || b[i] == b'$'
                        || b[i] >= 0x80)
                {
                    i += 1;
                }
                out.push((
                    Tok::Ident(String::from_utf8_lossy(&b[s..i]).into_owned()),
                    line,
                ));
            }
            _ if c.is_ascii_digit() => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_')
                {
                    i += 1;
                }
                out.push((Tok::Value, line));
            }
            b'.' if b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) => {
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push((Tok::Value, line));
            }
            _ => {
                out.push((Tok::Punct(c), line));
                i += 1;
            }
        }
    }
    out
}

/// A module's imports: static ones ([`RefKind::Import`], also `export …
/// from`) and `import("literal")` ([`RefKind::DynamicImport`]).
pub fn js(src: &str) -> Vec<Ref> {
    let t = js_tokens(src);
    let mut out = Vec::new();
    let ident = |k: usize, w: &str| matches!(t.get(k), Some((Tok::Ident(x), _)) if x == w);
    let punct = |k: usize, c: u8| matches!(t.get(k), Some((Tok::Punct(x), _)) if *x == c);
    let string = |k: usize| match t.get(k) {
        Some((Tok::Str(s), _)) => Some(s.clone()),
        _ => None,
    };
    // `… from "x"` after an import or export clause starting at `k`.
    let clause = |mut k: usize| -> Option<(String, u32)> {
        while k < t.len() {
            match &t[k].0 {
                Tok::Ident(w) if w == "from" => {
                    if let Some(s) = string(k + 1) {
                        return Some((s, t[k + 1].1));
                    }
                    k += 1;
                }
                Tok::Ident(w) if w == "import" || w == "export" => return None,
                Tok::Ident(_) => k += 1,
                Tok::Punct(b'*' | b',' | b'{' | b'}') => k += 1,
                _ => return None,
            }
        }
        None
    };
    for k in 0..t.len() {
        if punct(k.wrapping_sub(1), b'.') && k > 0 {
            continue;
        }
        if ident(k, "import") {
            if punct(k + 1, b'(') {
                if let Some(s) = string(k + 2) {
                    if punct(k + 3, b')') || punct(k + 3, b',') {
                        out.push(Ref {
                            url: s,
                            line: t[k + 2].1,
                            kind: RefKind::DynamicImport,
                        });
                    }
                }
            } else if let Some(s) = string(k + 1) {
                out.push(Ref {
                    url: s,
                    line: t[k + 1].1,
                    kind: RefKind::Import,
                });
            } else if !punct(k + 1, b'.') {
                if let Some((s, line)) = clause(k + 1) {
                    out.push(Ref {
                        url: s,
                        line,
                        kind: RefKind::Import,
                    });
                }
            }
        } else if ident(k, "export") && (punct(k + 1, b'*') || punct(k + 1, b'{')) {
            if let Some((s, line)) = clause(k + 1) {
                out.push(Ref {
                    url: s,
                    line,
                    kind: RefKind::Import,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(r: &[Ref]) -> Vec<(&str, u32, RefKind)> {
        r.iter().map(|r| (r.url.as_str(), r.line, r.kind)).collect()
    }

    #[test]
    fn html_references() {
        let h = html(concat!(
            "<!doctype html>\n",
            "<html><head>\n",
            "<link rel=\"stylesheet\" href=\"style.css?v=2\">\n",
            "<link rel=icon href='icon.png'><link rel=\"modulepreload\" href=\"/lib.js\">\n",
            "<script type=\"module\" src=\"main.js\"></script>\n",
            "<script src=\"https://cdn.example/x.js\"></script>\n",
            "<!-- <script src=\"commented.js\"></script> -->\n",
            "<script type=\"importmap\">{\"imports\":{\"a\":\"/a.js\"}}</script>\n",
            "<script type=module>\nimport { x } from './inline.js';\nconst s = \"</div>\";\n</script>\n",
            "<style>body { background: url(bg.png) }</style>\n",
            "</head><body>\n",
            "<img src=\"a&amp;b.png\" srcset=\"s1.png 1x, s2.png 2x\" style=\"background:url('st.png')\">\n",
            "<a href=\"../other.html#x\">x</a> <video poster=p.jpg><source src=v.mp4></video>\n",
            "<iframe src=\"tv.html\"></iframe><SCRIPT SRC=\"UP.JS\"></SCRIPT>\n",
            "</body></html>\n",
        ));
        assert_eq!(
            urls(&h.refs),
            [
                ("style.css?v=2", 3, RefKind::Stylesheet),
                ("icon.png", 4, RefKind::Asset),
                ("/lib.js", 4, RefKind::ModulePreload),
                ("main.js", 5, RefKind::Script { module: true }),
                (
                    "https://cdn.example/x.js",
                    6,
                    RefKind::Script { module: false }
                ),
                ("a&b.png", 15, RefKind::Asset),
                ("s1.png", 15, RefKind::Asset),
                ("s2.png", 15, RefKind::Asset),
                ("st.png", 15, RefKind::Asset),
                ("../other.html#x", 16, RefKind::Link),
                ("p.jpg", 16, RefKind::Asset),
                ("v.mp4", 16, RefKind::Asset),
                ("tv.html", 17, RefKind::Frame),
                ("UP.JS", 17, RefKind::Script { module: false }),
            ]
        );
        assert_eq!(h.import_maps.len(), 1);
        assert_eq!(h.scripts.len(), 1);
        assert!(h.scripts[0].module);
        assert_eq!(h.scripts[0].line, 9);
        assert_eq!(
            urls(&js(&h.scripts[0].text)),
            [("./inline.js", 2, RefKind::Import)]
        );
        assert_eq!(h.styles.len(), 1);
        assert_eq!(
            urls(&css(&h.styles[0].text)),
            [("bg.png", 1, RefKind::Asset)]
        );
    }

    #[test]
    fn css_references() {
        let c = css(concat!(
            "@import \"base.css\";\n",
            "@import url(theme.css) screen;\n",
            "/* url(no.png) */\n",
            "@font-face { src: url( \"f.woff2\" ) format(\"woff2\"), url(f.woff); }\n",
            ".a { background: URL(data:image/png;base64,AAA) }\n",
            ".b::after { content: \"url(not.png)\" }\n",
        ));
        assert_eq!(
            urls(&c),
            [
                ("base.css", 1, RefKind::Stylesheet),
                ("theme.css", 2, RefKind::Stylesheet),
                ("f.woff2", 4, RefKind::Asset),
                ("f.woff", 4, RefKind::Asset),
                ("data:image/png;base64,AAA", 5, RefKind::Asset),
            ]
        );
    }

    #[test]
    fn js_imports() {
        let r = js(concat!(
            "import { Party } from \"/_tvpipe/lib/index.js\";\n",
            "import def, * as ns from './a.js'\n",
            "import './side.js';\n",
            "export { x as y } from \"./re.js\";\n",
            "export * from './all.js';\n",
            "export const z = 1;\n",
            "// import { no } from './comment.js';\n",
            "const s = \"import x from './string.js'\";\n",
            "const t = `import ${ { a: 1 }.a } from './template.js'`;\n",
            "const r = /import x from '.\\/regex.js'/g, q = a / 2 / 3;\n",
            "const lazy = await import(\"./lazy.js\");\n",
            "const d = import(name);\n",
            "console.log(import.meta.url, obj.import('x.js'));\n",
            "import {\n  a,\n  b,\n} from './multi.js';\n",
        ));
        assert_eq!(
            urls(&r),
            [
                ("/_tvpipe/lib/index.js", 1, RefKind::Import),
                ("./a.js", 2, RefKind::Import),
                ("./side.js", 3, RefKind::Import),
                ("./re.js", 4, RefKind::Import),
                ("./all.js", 5, RefKind::Import),
                ("./lazy.js", 11, RefKind::DynamicImport),
                ("./multi.js", 17, RefKind::Import),
            ]
        );
    }

    #[test]
    fn never_panics() {
        for s in [
            "<",
            "<a",
            "<a href",
            "<a href=",
            "<a href=\"",
            "<script>",
            "<style",
            "<!--",
            "&#x110000;",
            "`${",
            "`${}",
            "}}}`",
            "/",
            "/[",
            "'",
            "import",
            "import(",
            "url(",
            "@import",
            "\\",
        ] {
            let _ = html(s);
            let _ = css(s);
            let _ = js(s);
            let _ = decode_entities(s);
        }
        assert_eq!(
            decode_entities("a&amp;b&#47;&#x41;&bogus;&"),
            "a&b/A&bogus;&"
        );
    }
}
