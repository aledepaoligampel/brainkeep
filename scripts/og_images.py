#!/usr/bin/env python3
"""Render the brainkeep.dev share images (site/og-<lang>.png, 1200x630) with headless Chrome.

Usage: python3 scripts/og_images.py

Run it after changing site-src/og.html or the headline copy, then commit the PNGs. The card
is served from site/ on a local port so it uses the site's own fonts; nothing is fetched
from the network. Set CHROME to the browser binary if it is not in the default macOS path.
"""
import http.server
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHROME = os.environ.get("CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")

spec = importlib.util.spec_from_file_location("build_site", ROOT / "scripts" / "build_site.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


def main():
    if not Path(CHROME).exists():
        print(f"Chrome not found at {CHROME}; set CHROME.")
        return 1
    template = (build.SRC / "og.html").read_text(encoding="utf-8")
    cards = {}
    for lang in build.LANGS:
        ctx = json.loads((build.SRC / "i18n" / f"{lang}.json").read_text(encoding="utf-8"))
        ctx["lang"] = lang
        cards[f"/og-card-{lang}.html"] = build.render(template, ctx, {}).encode()

    class Handler(http.server.SimpleHTTPRequestHandler):
        def do_GET(self):
            if self.path in cards:
                body = cards[self.path]
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            super().do_GET()

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), partial(Handler, directory=str(build.SITE)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for lang in build.LANGS:
            out = build.SITE / f"og-{lang}.png"
            with tempfile.TemporaryDirectory() as profile:
                subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
                                f"--user-data-dir={profile}", "--window-size=1200,630", "--virtual-time-budget=3000",
                                f"--screenshot={out}", f"http://127.0.0.1:{server.server_port}/og-card-{lang}.html"],
                               check=True, capture_output=True)
            print("wrote", out.relative_to(ROOT))
    finally:
        server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
