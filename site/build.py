#!/usr/bin/env python3
"""Builds the tvpipe website from Markdown.

    python3 site/build.py [--out DIR]

Sources:
  site/index.md          the landing page
  site/manual/NN-*.md    the manual, one section per file, in file order
  site/static/           CSS, JS and the logo, copied as is; install.sh and
                         install.ps1 are also copied to the site's root
  crates/tvpipe/src/browser/fonts/  the fonts (shared with tvpipe's share page)
  site/shots/            screenshots (site/screenshots.mjs makes them)
Output: site/_out/ (static files, serve the directory). Every page is a
directory with an index.html (manual/receivers/index.html), and links
point at the directory (manual/receivers/).

Only the Python standard library. The Markdown is a small subset, parsed
here; see "Syntax" below.

Syntax
------
Front matter between `---` lines, `key: value` per line. `spec:` takes
indented `Key: value` lines for the box under the section number; `;`
splits a value into several lines.

Blocks: paragraphs, `## section`, `### heading`, ``` fences (sh lines
starting with `$ ` are commands, `# ` comments, others output), pipe
tables, `-` and `1.` lists, `> ` notes, an image alone on a line (a
figure; its title is the caption; `{.crt}` after it shows it on a CRT),
raw HTML (a block starting with `<`), and containers. A figure NAME.png
with a NAME.dark.png next to it shows the dark one in the dark theme.

Containers:

  :::aside LABEL ... :::     margin note beside the current section
  :::caution ... :::         a margin note labelled Caution
  :::steps ... :::           a procedure; each `###` starts a step
  :::features ... :::        a grid; each `###` starts a tile
  :::buttons ... :::         one `[text](url)` per line
  :::install :::             the install commands, for the visitor's platform
  :::figure CAPTION ... :::  raw HTML (an inline SVG) as a numbered figure
  :::index :::               the list of manual sections

Inline: `code`, **bold**, *italic*, [links](receivers.md) (a manual page by its
slug, without the number, so renumbering does not break links).
"""

import html
import posixpath
import re
import shutil
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parent
INSTALLERS = ("install.sh", "install.ps1")
SOURCE = "https://github.com/mitsuhiko/tvpipe"
LICENSE = SOURCE + "/blob/main/LICENSE"
FOOTER_NOTE = f'tvpipe is open source. <a href="{LICENSE}">License</a>'
# The colour theme switch (site.js), in the footer of every page.
THEME = """<div class="theme" role="group" aria-label="Colour theme">  <button data-t="light" title="Light" aria-label="Light"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="2.75"/><path d="M8 1.25v1.5M8 13.25v1.5M1.25 8h1.5M13.25 8h1.5M3.2 3.2l1.06 1.06M11.74 11.74l1.06 1.06M3.2 12.8l1.06-1.06M11.74 4.26l1.06-1.06"/></svg></button>  <button data-t="dark" title="Dark" aria-label="Dark"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13.6 10.2A6 6 0 0 1 5.8 2.4a6 6 0 1 0 7.8 7.8z"/></svg></button>  <button data-t="auto" title="Like the system" aria-label="Like the system"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5.75"/><path class="fill" d="M8 2.25a5.75 5.75 0 0 1 0 11.5z"/></svg></button></div>"""


# ---------------------------------------------------------------- inline

def inline(text, ctx):
    """Inline Markdown to HTML. Code spans are cut out first so nothing
    inside them is interpreted."""
    codes = []

    def keep_code(m):
        codes.append("<code>" + html.escape(m.group(1), quote=False) + "</code>")
        return f"\x00{len(codes) - 1}\x00"

    text = re.sub(r"`([^`]+)`", keep_code, text)
    text = html.escape(text, quote=False)
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", text)

    def link(m):
        label, href = m.group(1), m.group(2)
        return f'<a href="{html.escape(ctx.href(href))}">{label}</a>'

    text = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, text)
    return re.sub(r"\x00(\d+)\x00", lambda m: codes[int(m.group(1))], text)


# ---------------------------------------------------------------- code

# Build-time syntax highlighting: a token regex per language, the first
# alternative that matches wins. Languages: shell and PowerShell, Rust,
# JavaScript and TypeScript, JSON. Classes: k keyword, s string, n number or
# type, c comment, f flag, v variable, p prompt, o output, x command.

def _rules(*rules):
    return re.compile("|".join(f"(?P<{name}>{rx})" for name, rx in rules), re.M)


RUST = _rules(
    ("c", r"//[^\n]*|/\*[\s\S]*?\*/"),
    ("s", r'b?"(?:\\.|[^"\\])*"|' + r"'(?:\\.|[^\\'])'"),
    ("c2", r"#!?\[[^\]\n]*\]"),
    ("k", r"\b(?:as|async|await|break|const|continue|crate|dyn|else|enum|extern|false|fn|for|if|impl|in|let|loop|match|mod|move|mut|pub|ref|return|self|Self|static|struct|super|trait|true|type|unsafe|use|where|while)\b|\b[a-z_][a-z0-9_]*!"),
    ("n", r"\b(?:u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize|f32|f64|bool|char|str)\b|\b[A-Z][A-Za-z0-9_]*\b|\b0x[0-9a-fA-F_]+\b|\b\d[\d_]*(?:\.\d+)?\b"),
)
JS = _rules(
    ("c", r"//[^\n]*|/\*[\s\S]*?\*/"),
    ("s", r'"(?:\\.|[^"\\\n])*"|' + r"'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`"),
    ("k", r"\b(?:as|async|await|break|case|catch|class|const|continue|default|delete|do|else|export|extends|false|finally|for|from|function|if|import|in|instanceof|interface|let|new|null|of|return|static|super|switch|this|throw|true|try|type|typeof|undefined|using|var|void|while|yield)\b"),
    ("n", r"\b[A-Z][A-Za-z0-9_]*\b|\b\d[\d_]*(?:\.\d+)?\b"),
)
SHELL = _rules(
    ("s", r'"(?:\\.|[^"\\])*"|' + r"'[^']*'"),
    ("c", r"(?<=\s)#[^\n]*"),
    ("v", r"\$\{[^}\n]*\}|\$env:\w+|\$\w+|\b[A-Z][A-Z0-9_]*(?==)"),
    ("f", r"(?<![\w-])--?[A-Za-z][\w-]*"),
)
JSON = _rules(
    ("v", r'"(?:\\.|[^"\\\n])*"(?=\s*:)'),
    ("s", r'"(?:\\.|[^"\\\n])*"'),
    ("k", r"\b(?:true|false|null)\b"),
    ("n", r"-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b"),
)
LANGS = {"json": JSON, "rust": RUST, "js": JS, "javascript": JS, "ts": JS, "typescript": JS,
         "sh": SHELL, "shell": SHELL, "console": SHELL, "powershell": SHELL}
PROMPTS = ("$ ", "PS> ")


def highlight(rules, code):
    out, pos = [], 0
    for m in rules.finditer(code):
        out.append(html.escape(code[pos:m.start()], quote=False))
        out.append(f'<span class="{m.lastgroup[0]}">{html.escape(m.group(), quote=False)}</span>')
        pos = m.end()
    out.append(html.escape(code[pos:], quote=False))
    return "".join(out)


def shell_line(line, rules=SHELL):
    """A command line: the prompt, the command in bold, the rest highlighted.
    Lines without a prompt are a command's output, or a comment."""
    prompt = next((p for p in PROMPTS if line.startswith(p)), None)
    if prompt is None:
        if line.startswith("#"):
            return f'<span class="c">{html.escape(line, quote=False)}</span>'
        return f'<span class="o">{html.escape(line, quote=False)}</span>' if line.strip() else ""
    rest = line[len(prompt):]
    word, _, tail = rest.partition(" ")
    return (f'<span class="p">{html.escape(prompt, quote=False)}</span>'
            f'<span class="x">{html.escape(word, quote=False)}</span>'
            + (" " + highlight(rules, tail) if tail or rest.endswith(" ") else ""))


def code_block(lang, lines):
    rules = LANGS.get(lang)
    if rules is SHELL:
        body = "\n".join(shell_line(line) for line in lines)
    elif rules is not None:
        body = highlight(rules, "\n".join(lines))
    else:
        body = html.escape("\n".join(lines), quote=False)
    cls = f' class="lang-{lang}"' if lang else ""
    return f"<pre{cls}><code>{body}</code></pre>"


# The install commands; site.js shows the visitor's platform first.
def install_html():
    return ("""<div class="install" data-install>
<div class="tabs" role="tablist"><button role="tab" data-os="unix" aria-selected="true">macOS and Linux</button><button role="tab" data-os="windows" aria-selected="false">Windows</button></div>
<pre data-os="unix"><code>""" + shell_line("$ curl -fsSL https://mitsuhiko.github.io/tvpipe/install.sh | sh") + """</code><button class="copy" aria-label="Copy">Copy</button></pre>
<pre data-os="windows" hidden><code>""" + shell_line("PS> irm https://mitsuhiko.github.io/tvpipe/install.ps1 | iex") + """</code><button class="copy" aria-label="Copy">Copy</button></pre>
</div>""")


# ---------------------------------------------------------------- blocks

def rel(here, target):
    """The relative URL from directory `here` to directory `target` (both
    relative to the site root, "" or ending in "/"), ending in "/"."""
    r = posixpath.relpath("/" + target, "/" + here)
    return "./" if r == "." else r + "/"


class Ctx:
    """Per-page state: link rewriting, figure numbers, margin notes."""

    def __init__(self, here, num, pages):
        self.here = here          # the page's directory: "", "manual/", "manual/play/"
        self.root = rel(here, "")  # the way back to the site root, "" or "../.."
        self.num = num            # section number for figures ("05"), or None
        self.pages = pages
        self.figures = 0
        self.asides = []

    def to(self, target):
        """A link from this page to the directory `target`."""
        return rel(self.here, target)

    def href(self, href):
        if re.match(r"^[a-z]+:", href) or href.startswith("#"):
            return href
        path, _, frag = href.partition("#")
        if path.endswith(".md"):
            # A page by its slug: receivers.md (or 04-receivers.md) is manual/receivers/.
            slug = re.sub(r"^\d\d-", "", Path(path).stem)
            name = Path(path).name
            page = next((p for p in self.pages if p["slug"] == slug), None)
            if page is None and name != "index.md":
                raise SystemExit(f"link to unknown page {href}")
            path = self.to(page["dir"] if page else "")
        return path + ("#" + frag if frag else "")

    def fig(self):
        self.figures += 1
        return f"Fig. {self.num}.{self.figures}" if self.num else f"Fig. {self.figures}"


IMG_RE = re.compile(r'^!\[([^\]]*)\]\(([^)\s]+)(?:\s+"([^"]*)")?\)(\{\.crt\})?\s*$')
TABLE_SEP = re.compile(r"^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$")
LIST_RE = re.compile(r"^(\s*)([-*]|\d+\.)\s+(.*)$")


def split_row(line):
    line = line.strip()
    if line.startswith("|"):
        line = line[1:]
    if line.endswith("|"):
        line = line[:-1]
    cells, cur, in_code = [], "", False
    for ch in line:
        if ch == "`":
            in_code = not in_code
        if ch == "|" and not in_code:
            cells.append(cur.strip())
            cur = ""
        else:
            cur += ch
    cells.append(cur.strip())
    return cells


def figure(inner, caption, ctx, cls=""):
    label = ctx.fig()
    cap = f'<figcaption><span class="lbl">{label}</span><span>{inline(caption, ctx)}</span></figcaption>' if caption else ""
    return f'<figure{cls}>{inner}{cap}</figure>'


def render(lines, ctx):
    """Block Markdown to HTML. Margin notes go to ctx.asides."""
    out, i = [], 0
    while i < len(lines):
        line = lines[i]
        s = line.strip()
        if not s:
            i += 1
            continue

        # containers
        if s.startswith(":::"):
            kind, _, arg = s[3:].strip().partition(" ")
            depth, j = 1, i + 1
            while j < len(lines):
                t = lines[j].strip()
                if t.startswith(":::") and t != ":::":
                    depth += 1
                elif t == ":::":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            body = lines[i + 1:j]
            i = j + 1
            out.append(container(kind, arg.strip(), body, ctx))
            continue

        # fenced code
        if s.startswith("```"):
            lang = s[3:].strip()
            j = i + 1
            while j < len(lines) and not lines[j].strip().startswith("```"):
                j += 1
            out.append(code_block(lang, lines[i + 1:j]))
            i = j + 1
            continue

        # headings
        if s.startswith("### "):
            out.append(f"<h3>{inline(s[4:], ctx)}</h3>")
            i += 1
            continue

        # figure
        m = IMG_RE.match(s)
        if m:
            alt, src, cap, crt = m.groups()
            src = ctx.root + src
            if crt:
                inner = (f'<div class="crt" data-crt><canvas aria-hidden="true"></canvas>'
                         f'<img src="{src}" alt="{html.escape(alt)}"></div>')
            else:
                # A NAME.dark.png next to NAME.png is shown in the dark theme.
                dark = Path(m.group(2)).with_suffix(".dark" + Path(m.group(2)).suffix)
                imgs = f'<img src="{src}" alt="{html.escape(alt)}" loading="lazy">'
                if (SITE / dark).exists():
                    imgs = (f'<img class="light" src="{src}" alt="{html.escape(alt)}" loading="lazy">'
                            f'<img class="dark" src="{ctx.root}{dark}" alt="{html.escape(alt)}" loading="lazy">')
                inner = f'<div class="frame">{imgs}</div>'
            out.append(figure(inner, cap or "", ctx))
            i += 1
            continue

        # raw HTML
        if s.startswith("<"):
            j = i
            while j < len(lines) and lines[j].strip():
                j += 1
            out.append("\n".join(lines[i:j]))
            i = j
            continue

        # table
        if s.startswith("|") and i + 1 < len(lines) and TABLE_SEP.match(lines[i + 1].strip()):
            head = split_row(s)
            j, rows = i + 2, []
            while j < len(lines) and lines[j].strip().startswith("|"):
                rows.append(split_row(lines[j]))
                j += 1
            t = "<table><thead><tr>" + "".join(f"<th>{inline(h, ctx)}</th>" for h in head) + "</tr></thead><tbody>"
            for r in rows:
                t += "<tr>" + "".join(f"<td>{inline(c, ctx)}</td>" for c in r) + "</tr>"
            out.append('<div class="table">' + t + "</tbody></table></div>")
            i = j
            continue

        # note
        if s.startswith(">"):
            j, buf = i, []
            while j < len(lines) and lines[j].strip().startswith(">"):
                buf.append(lines[j].strip()[1:].strip())
                j += 1
            text = " ".join(buf)
            m = re.match(r"^\*\*(\w+)\.\*\*\s*(.*)$", text)
            label, text = (m.group(1), m.group(2)) if m else ("Note", text)
            out.append(f'<div class="note"><span class="lbl">{html.escape(label)}</span><p>{inline(text, ctx)}</p></div>')
            i = j
            continue

        # list
        m = LIST_RE.match(line)
        if m:
            ordered = m.group(2)[0].isdigit()
            items, j = [], i
            while j < len(lines):
                m = LIST_RE.match(lines[j])
                if m and len(m.group(1)) == 0:
                    items.append(m.group(3))
                elif lines[j].strip() and lines[j].startswith("  ") and items:
                    items[-1] += " " + lines[j].strip()
                else:
                    break
                j += 1
            tag = "ol" if ordered else "ul"
            out.append(f'<{tag} class="list">' + "".join(f"<li>{inline(t, ctx)}</li>" for t in items) + f"</{tag}>")
            i = j
            continue

        # paragraph
        j, buf = i, []
        while j < len(lines):
            t = lines[j].strip()
            if not t or t.startswith(("```", ":::", "### ", "## ", ">", "|")) or IMG_RE.match(t) or (LIST_RE.match(lines[j]) and j > i):
                break
            buf.append(t)
            j += 1
        out.append(f"<p>{inline(' '.join(buf), ctx)}</p>")
        i = j
    return "\n".join(out)


def split_at_h3(lines):
    """[(title, body_lines)] for containers whose items start at `###`."""
    items = []
    for line in lines:
        if line.strip().startswith("### "):
            items.append([line.strip()[4:], []])
        elif items:
            items[-1][1].append(line)
    return items


def container(kind, arg, body, ctx):
    if kind in ("aside", "caution"):
        label = "Caution" if kind == "caution" else arg
        cls = ' class="lbl caution"' if kind == "caution" else ' class="lbl"'
        head = f"<span{cls}>{html.escape(label)}</span>" if label else ""
        ctx.asides.append(head + render(body, ctx))
        return ""
    if kind == "steps":
        lis = "".join(f"<li><div><h3>{inline(t, ctx)}</h3>{render(b, ctx)}</div></li>" for t, b in split_at_h3(body))
        return f'<ol class="steps">{lis}</ol>'
    if kind == "features":
        tiles = "".join(
            f'<div class="feature"><span class="n">{n:02}</span><h3>{inline(t, ctx)}</h3>{render(b, ctx)}</div>'
            for n, (t, b) in enumerate(split_at_h3(body), 1))
        return f'<div class="features">{tiles}</div>'
    if kind == "buttons":
        links = re.findall(r"\[([^\]]+)\]\(([^)\s]+)\)", "\n".join(body))
        return '<div class="buttons">' + "".join(
            f'<a class="btn{" primary" if n == 0 else ""}" href="{html.escape(ctx.href(h))}">{html.escape(t)}</a>'
            for n, (t, h) in enumerate(links)) + "</div>"
    if kind == "install":
        return install_html()
    if kind == "figure":
        return figure('<div class="frame">' + "\n".join(body) + "</div>", arg, ctx)
    if kind == "index":
        return '<div class="index">' + "".join(
            f'<a href="{ctx.to(p["dir"])}"><span class="n">{p["num"]}</span><span><b>{html.escape(p["title"])}</b>'
            f'<span>{html.escape(p.get("summary", ""))}</span></span></a>'
            for p in ctx.pages[1:]) + "</div>"
    raise SystemExit(f"unknown container :::{kind}")


# ---------------------------------------------------------------- pages

def read_md(path):
    text = path.read_text(encoding="utf-8")
    meta, spec = {}, []
    if text.startswith("---\n"):
        head, _, text = text[4:].partition("\n---\n")
        key = None
        for line in head.splitlines():
            if line.startswith((" ", "\t")) and key == "spec":
                k, _, v = line.strip().partition(":")
                spec.append((k.strip(), v.strip()))
            elif ":" in line:
                key, _, v = line.partition(":")
                key = key.strip()
                meta[key] = v.strip()
    meta["spec"] = spec
    return meta, text.splitlines()


def spec_html(spec, ctx):
    if not spec:
        return ""
    rows = ""
    for k, v in spec:
        for n, part in enumerate(v.split("; ")):
            rows += f"<dt>{html.escape(k) if n == 0 else ''}</dt><dd>{inline(part, ctx)}</dd>"
    return f'<dl class="spec">{rows}</dl>'


def manual_page(page, ctx):
    """Header (number, title, lede, spec) and one row per `##` section."""
    lines = page["lines"]
    sections, cur = [], ("", [])
    for line in lines:
        if line.startswith("## "):
            sections.append(cur)
            cur = (line[3:].strip(), [])
        else:
            cur[1].append(line)
    sections.append(cur)

    ctx.asides = []
    lede = render(sections[0][1], ctx)
    head = (f'<header class="row ph"><div class="meta"><span class="num">{page["num"]}</span>'
            f'{spec_html(page["spec"], ctx)}{"".join(ctx.asides)}</div>'
            f'<div class="body"><h1>{html.escape(page["title"])}</h1><div class="lede">{lede}</div></div></header>')
    rows = []
    for n, (title, body) in enumerate(sections[1:], 1):
        ctx.asides = []
        inner = render(body, ctx)
        meta = "".join(ctx.asides)
        rows.append(f'<section class="row sec" id="s{n}"><div class="meta">{meta}</div><div class="body">'
                    f'<h2><span class="sn">{page["num"]}.{n}</span>{inline(title, ctx)}</h2>{inner}</div></section>')
    return head + "\n".join(rows)


def nav_html(pages, current, ctx):
    groups, out = [], []
    for p in pages:
        g = p.get("group", "")
        if not groups or groups[-1][0] != g:
            groups.append((g, []))
        groups[-1][1].append(p)
    for g, ps in groups:
        out.append(f'<h4 class="lbl">{html.escape(g)}</h4><ol>')
        for p in ps:
            cur = ' aria-current="page"' if p is current else ""
            out.append(f'<li><a href="{ctx.to(p["dir"])}"{cur}><span class="n">{p["num"]}</span>{html.escape(p["title"])}</a></li>')
        out.append("</ol>")
    return "".join(out)


def layout(title, crumb, body, root, kind):
    tpl = (SITE / "templates" / "layout.html").read_text(encoding="utf-8")
    for k, v in {
        "title": html.escape(title), "crumb": crumb, "body": body, "root": root,
        "kind": kind, "source": SOURCE,
    }.items():
        tpl = tpl.replace("{{" + k + "}}", v)
    return tpl


def write(out, here, text):
    """Writes the page for directory `here` (its index.html)."""
    path = out / here / "index.html"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def build(out):
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    shutil.copytree(SITE / "static", out / "static")
    # The fonts are the ones compiled into tvpipe for its share page.
    shutil.copytree(SITE.parent / "crates" / "tvpipe" / "src" / "browser" / "fonts", out / "static" / "fonts")
    # The installers live at the root, for the one-liners:
    # curl -fsSL https://mitsuhiko.github.io/tvpipe/install.sh | sh
    for name in INSTALLERS:
        if (SITE / "static" / name).exists():
            shutil.copy(SITE / "static" / name, out / name)
    if (SITE / "shots").exists():
        shutil.copytree(SITE / "shots", out / "shots")

    pages = []
    for path in sorted((SITE / "manual").glob("[0-9][0-9]-*.md")):
        meta, lines = read_md(path)
        num, slug = path.stem.split("-", 1)
        meta.update(file=path.name, num=num, slug=slug, lines=lines,
                    dir="manual/" if num == "01" else f"manual/{slug}/")
        pages.append(meta)

    for i, page in enumerate(pages):
        ctx = Ctx(page["dir"], page["num"], pages)
        main = manual_page(page, ctx)
        prev, nxt = (pages[i - 1] if i else None), (pages[i + 1] if i + 1 < len(pages) else None)
        pn = "".join(f'<a href="{ctx.to(p["dir"])}"><span>{lbl}</span>{html.escape(p["title"])}</a>'
                     for lbl, p in (("Previous", prev), ("Next", nxt)) if p)
        body = (f'<div class="shell"><nav class="side" id="nav" aria-label="Contents">{nav_html(pages, page, ctx)}</nav>'
                f'<main>{main}<footer class="pf"><div>tvpipe Operations Manual<br>{FOOTER_NOTE}</div>'
                f'<div class="pn">{pn}</div><div class="r">{THEME}</div></footer></main></div>')
        crumb = f'Section {page["num"]}<span>{html.escape(page["title"])}</span>'
        title = ("" if i == 0 else page["title"] + " · ") + "tvpipe operations manual"
        write(out, page["dir"], layout(title, crumb, body, ctx.root, "manual"))

    meta, lines = read_md(SITE / "index.md")
    ctx = Ctx("", None, pages)
    body = f'<main class="landing">{render(lines, ctx)}</main>'
    body += f'<footer class="lf"><div>{FOOTER_NOTE}</div><div class="r">{THEME}</div></footer>'
    write(out, "", layout(meta.get("title", "tvpipe"), "", body, ctx.root, "landing"))
    print(f"built {len(pages) + 1} pages into {out}")


if __name__ == "__main__":
    args = sys.argv[1:]
    out = Path(args[args.index("--out") + 1]) if "--out" in args else SITE / "_out"
    build(out.resolve())
