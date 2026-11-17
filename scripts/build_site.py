#!/usr/bin/env python3
"""Generate the brainkeep.dev landing in site/ from site-src/.

Usage: python3 scripts/build_site.py [--check]

site/ is served as plain files (Cloudflare Pages, no build on the host). This script
renders site-src/index.html once per language (site-src/i18n/<lang>.json), copies the
panel's design tokens and fonts so the site never drifts from the panel, and versions
the stylesheets and scripts by content. Commit its output. --check exits 1 when the
committed files are out of date.
"""
import hashlib
import html
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "site-src"
SITE = ROOT / "site"
THEME = ROOT / "panel-ui" / "src" / "theme.css"
FONTS = ROOT / "panel-ui" / "src" / "fonts"
ICON = ROOT / "panel-ui" / "public" / "icon.svg"
ORIGIN = "https://brainkeep.dev"
LANGS = {"en": "/", "es": "/es/"}

LOOP = re.compile(r"\{%\s*for (\w+) in ([\w.]+)\s*%\}(.*?)\{%\s*endfor\s*%\}", re.S)
HOLE = re.compile(r"\{\{\s*([\w.:-]+)\s*\}\}")


def version():
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    return re.search(r'^version\s*=\s*"([^"]+)"', text, re.M).group(1)


def tokens_css():
    """The panel's @font-face rules and light/dark tokens, with font URLs rebased."""
    head = THEME.read_text(encoding="utf-8").split("*, *::before")[0].rstrip() + "\n"
    head = head.replace("url('./fonts/", "url('/assets/fonts/")
    return ("/* Generated from panel-ui/src/theme.css by scripts/build_site.py. Do not edit. */\n" + head).encode()


def lookup(ctx, path):
    value = ctx
    for part in path.split("."):
        if not isinstance(value, dict) or part not in value:
            raise KeyError(f"unknown template key: {path}")
        value = value[part]
    if isinstance(value, (dict, list)):
        raise TypeError(f"template key is not text: {path}")
    return str(value)


def render(template, ctx, assets):
    """Fill {{key}} holes and {% for x in key %} loops (not nested). Each value is escaped and
    substituted once: text coming from a value is never scanned for holes again."""
    rendered = []

    def loop(match):
        name, path, body = match.groups()
        items = ctx
        for part in path.split("."):
            items = items[part]
        if not isinstance(items, list):
            raise TypeError(f"template loop needs a list: {path}")
        rendered.append("".join(render(body, {**ctx, name: item}, assets) for item in items))
        return f"\x00{len(rendered) - 1}\x00"

    def hole(match):
        key = match.group(1)
        if key.startswith("asset:"):
            return assets[key[6:]]
        return html.escape(lookup(ctx, key), quote=True)

    out = HOLE.sub(hole, LOOP.sub(loop, template))
    return re.sub(r"\x00(\d+)\x00", lambda m: rendered[int(m.group(1))], out)


def outputs():
    files = {"assets/tokens.css": tokens_css(), "favicon.svg": ICON.read_bytes()}
    for font in sorted(FONTS.glob("*.woff2")):
        files["assets/fonts/" + font.name] = font.read_bytes()

    def asset(name):
        data = files.get("assets/" + name) or (SITE / "assets" / name).read_bytes()
        return f"/assets/{name}?v={hashlib.sha256(data).hexdigest()[:10]}"

    assets = {name: asset(name) for name in ("tokens.css", "site.css", "site.js", "theme.js")}
    template = (SRC / "index.html").read_text(encoding="utf-8")
    for lang, path in LANGS.items():
        ctx = json.loads((SRC / "i18n" / f"{lang}.json").read_text(encoding="utf-8"))
        ctx.update(lang=lang, path=path, origin=ORIGIN, version=version(),
                   langs=[{"code": code, "href": href, "label": code.upper(),
                           "current": "page" if code == lang else "false"} for code, href in LANGS.items()])
        page = render(template, ctx, assets)
        if "{{" in page or "{%" in page:
            raise ValueError(f"{lang}: unrendered template markers")
        files[("" if path == "/" else path.strip("/") + "/") + "index.html"] = page.encode()
    not_found = NOT_FOUND
    for name, url in assets.items():
        not_found = not_found.replace("{" + name + "}", url)
    files["404.html"] = not_found.encode()
    return files


# Cloudflare Pages serves 404.html for unknown paths (without it, it would serve index.html).
NOT_FOUND = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Not found — Brainkeep</title>
<meta name="robots" content="noindex">
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="stylesheet" href="{tokens.css}">
<link rel="stylesheet" href="{site.css}">
<script src="{theme.js}"></script>
</head>
<body>
<main class="notfound">
<p class="label accent">404</p>
<h1 class="h2">Not found <span lang="es">· No encontrada</span></h1>
<p class="body-lg">This page does not exist. <span lang="es">Esta página no existe.</span></p>
<div class="actions center"><a class="btn primary big" href="/">brainkeep.dev</a><a class="btn big" href="/es/" hreflang="es" lang="es">Español</a></div>
</main>
</body>
</html>
"""


def main(argv):
    check = "--check" in argv
    stale = []
    files = outputs()
    # Generated folders hold only generated files: a font removed from the panel must not stay published.
    for orphan in sorted((SITE / "assets" / "fonts").glob("*")):
        name = "assets/fonts/" + orphan.name
        if name not in files:
            stale.append(name)
            if not check:
                orphan.unlink()
    for name, data in files.items():
        target = SITE / name
        if target.is_file() and target.read_bytes() == data:
            continue
        stale.append(name)
        if not check:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
    if check and stale:
        print("site/ is out of date; run python3 scripts/build_site.py: " + ", ".join(stale))
        return 1
    if not check:
        print("site/ up to date" if not stale else "updated " + ", ".join(stale))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
