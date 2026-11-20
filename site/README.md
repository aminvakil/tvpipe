# Website

The landing page and the operations manual, built from Markdown into static
HTML.

```sh
python3 site/build.py                  # → site/_out/
python3 -m http.server -d site/_out    # look at it
```

| Path | |
|---|---|
| `index.md` | the landing page |
| `manual/NN-slug.md` | one manual section per file, numbered by its prefix, built to `manual/slug/index.html` |
| `templates/layout.html` | the page shell: header, logo symbol, scripts |
| `static/` | `style.css`, `site.js` (theme switch, navigation without reloads, CRT figure), the logo |
| `../crates/tvpipe/src/browser/fonts/` | the fonts, shared with tvpipe's share page (OFL-1.1, `LICENSES/OFL-1.1-fonts.txt`) |
| `static/install.sh`, `static/install.ps1` | the installers, also copied to the site's root (`/install.sh`, `/install.ps1`) |
| `shots/` | screenshots, made by `node site/screenshots.mjs` |
| `build.py` | the generator: standard-library Python, its Markdown subset is described at the top |

The generator has no dependencies. Its Markdown is a small subset plus a few
containers for the manual's layout (`:::aside`, `:::caution`, `:::steps`,
`:::features`, `:::figure`); `build.py`'s docstring lists them.

## Screenshots

`node site/screenshots.mjs [path/to/tvpipe]` runs `tvpipe preview` and
`tvpipe share --to browser`, opens their pages in headless Chrome and saves
the PNGs in `shots/`. Rerun it when the player or the share page changes.
Needs Node 22+ and Chrome.

## Publishing

The site lives at https://tvpipe.party/: `.github/workflows/tvpipe-party.yml`
builds it and deploys it with the Worker in `apps/tvpipe-party/` (which
also serves the short join links) on pushes to `main` that touch it, and
on manual dispatch. `.github/workflows/site.yml` still deploys the same
build to GitHub Pages (https://mitsuhiko.github.io/tvpipe/), so links and
installer commands from older releases keep working.
