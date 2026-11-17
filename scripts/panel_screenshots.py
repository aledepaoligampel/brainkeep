#!/usr/bin/env python3
"""Render the landing's panel screenshots (site-src/screenshots/panel-<lang>-<theme>.png)
from the real panel filled with fictional data.

Usage: python3 scripts/panel_screenshots.py

Per language it starts a throwaway server on a temporary data directory, invites the
fictional team from site-src/demo/<lang>.json through the panel API, saves their memories
over MCP, backdates the revisions so the list reads like a lived-in project, and captures
the panel with headless Chrome in light and dark. Run scripts/build_site.py afterwards to
publish them. Nothing touches ~/.brainkeep or the network. Build the binary first
(cargo build); set BRAINKEEP_BINARY or CHROME to override.
"""
import base64
import datetime
import http.client
import http.server
import json
import os
import secrets
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("BRAINKEEP_BINARY", ROOT / "target" / "debug" / "brainkeep"))
CHROME = os.environ.get("CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
HOST = "panel.example.test"
ORIGIN = "https://" + HOST
LANGS = tuple(os.environ.get("DEMO_LANGS", "en es").split())
THEMES = ("light", "dark")
WIDTH, HEIGHT = 1440, 900


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Server:
    def __init__(self, root, owner):
        self.root, self.owner, self.port = root, owner, free_port()
        self.agent_token = secrets.token_hex(32)
        self.admin_key = secrets.token_hex(32)
        for name, value in (("auth.json", {"demo": self.agent_token}),
                            ("panel.json", {"read": secrets.token_hex(32), "session_key": secrets.token_hex(32),
                                            "admin": self.admin_key})):
            path = root / name
            path.write_text(json.dumps(value))
            path.chmod(0o600)
        self.data = root / "data"
        self.proc = None

    def start(self):
        command = [str(BINARY), "--data-dir", str(self.data), "serve", "--http-port", str(self.port),
                   "--http-host", HOST, "--auth-file", str(self.root / "auth.json"),
                   "--panel-auth-file", str(self.root / "panel.json"), "--owner", self.owner]
        self.log = open(self.root / "log", "ab")
        self.proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=self.log)
        for _ in range(200):
            if self.proc.poll() is not None:
                raise SystemExit("server failed: " + (self.root / "log").read_text())
            try:
                if self.request("GET", "/panel/api/login")[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.05)
        raise SystemExit("server did not start")

    def stop(self):
        self.proc.terminate()
        self.proc.wait(timeout=10)
        self.log.close()

    def request(self, method, path, body=None, headers=None):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=15)
        c.request(method, path, body, {"Host": HOST, **(headers or {})})
        r = c.getresponse()
        out = (r.status, r.read(), r.getheaders())
        c.close()
        return out

    def login(self):
        status, body, headers = self.request("POST", "/panel/api/login", json.dumps({"token": self.admin_key}),
                                             {"Origin": ORIGIN, "Content-Type": "application/json"})
        assert status == 200, body
        cookie = next(v.split(";")[0] for k, v in headers if k.lower() == "set-cookie")
        status, body, _ = self.request("GET", "/panel/api/session", headers={"Cookie": cookie})
        return cookie, json.loads(body)["csrf"]

    def invite(self, cookie, csrf, email, handle):
        status, body, _ = self.request("POST", "/panel/api/invite", json.dumps({"email": email, "handle": handle}), {
            "Origin": ORIGIN, "Content-Type": "application/json", "Cookie": cookie, "X-CSRF-Token": csrf})
        assert status == 200, body
        return json.loads(body)["token"]

    def tool(self, token, name, args):
        rpc = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}
        status, body, _ = self.request("POST", "/mcp", json.dumps(rpc), {
            "Accept": "application/json, text/event-stream", "Content-Type": "application/json",
            "MCP-Protocol-Version": "2025-03-26", "Authorization": "Bearer " + token})
        assert status == 200, body
        result = json.loads(body)["result"]
        assert not result.get("isError"), result
        return json.loads(result["content"][0]["text"])["data"]


def seed(server, demo):
    """Save the demo memories; return [(id, revision, minutes ago)] and the focus memory's path."""
    owner = demo["owner"]["handle"]
    cookie, csrf = server.login()
    tokens = {owner: server.agent_token}
    emails = {}
    for person in demo["team"]:
        tokens[person["handle"]] = server.invite(cookie, csrf, person["email"], person["handle"])
        emails[person["handle"]] = person["email"]
    stamps, focus, shared = [], None, False
    # The owner's memories first: a project exists, and can be shared, once it has one.
    for memory in sorted(demo["memories"], key=lambda m: m["by"] != owner):
        by = memory["by"]
        if by != owner and not shared:
            for name, handles in demo["share"].items():
                for handle in handles:
                    server.tool(server.agent_token, "share", {"project": name, "email": emails[handle], "role": "write"})
            shared = True
        project = memory["project"] if by == owner else f"{owner}/{memory['project']}"
        revisions = memory.get("revisions") or [{"ago": memory["ago"], "agent": memory["agent"], "content": memory["content"]}]
        saved = None
        for n, rev in enumerate(revisions):
            args = {"project": project, "content": rev["content"], "title": memory["title"], "kind": memory["kind"],
                    "topic": memory["topic"], "source": "demo", "agent": rev.get("agent", memory["agent"]),
                    "operation_id": secrets.token_hex(8),
                    "confirmed": bool(memory.get("confirmed")) and n == len(revisions) - 1}
            if saved:
                args.update(id=saved["id"], expected_revision=saved["revision"])
                saved = server.tool(tokens[by], "correct", args)
            else:
                saved = server.tool(tokens[by], "remember", args)
            stamps.append((saved["id"], saved["revision"], rev["ago"]))
        if memory["title"] == demo["focus"]:
            focus = (memory["project"], saved["id"])
    return stamps, focus


def backdate(server, demo, stamps, now):
    db = sqlite3.connect(server.data / "memory.sqlite")
    with db:
        for memory_id, revision, ago in stamps:
            when = (now - datetime.timedelta(minutes=ago)).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"
            db.execute("UPDATE revisions SET saved_at=? WHERE id=? AND revision=?", (when, memory_id, revision))
            if revision == 1:
                db.execute("UPDATE memories SET created_at=? WHERE id=?", (when, memory_id))
        for person in [demo["owner"], *demo["team"]]:
            db.execute("UPDATE users SET name=? WHERE handle=?", (person["name"], person["handle"]))
    db.close()


class Proxy(http.server.ThreadingHTTPServer):
    """Plain-HTTP front for Chrome: adds the panel session and the HTTPS Host/Origin."""

    def __init__(self, server, cookie):
        self.upstream, self.cookie = server, cookie
        super().__init__(("127.0.0.1", 0), ProxyHandler)


class ProxyHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/demo-setup?"):
            # Same origin as the panel: store its own language and theme preferences, then open it.
            q = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
            prefs = {"brainkeep.lang": q["lang"][0], "brainkeep.theme": q["theme"][0]}
            body = ("<!doctype html><script>" + "".join(
                f"localStorage.setItem({json.dumps(k)},{json.dumps(v)});" for k, v in prefs.items())
                + f"location.replace({json.dumps(q['next'][0])});</script>").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        up = self.server.upstream
        c = http.client.HTTPConnection("127.0.0.1", up.port, timeout=30)
        c.request("GET", self.path, headers={"Host": HOST, "Cookie": self.server.cookie,
                                             "Accept": self.headers.get("Accept", "*/*")})
        r = c.getresponse()
        self.send_response(r.status)
        for k, v in r.getheaders():
            if k.lower() not in ("transfer-encoding", "connection", "set-cookie", "strict-transport-security"):
                self.send_header(k, v)
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            while chunk := r.read1(65536):
                self.wfile.write(chunk)
                self.wfile.flush()
        except OSError:
            pass
        c.close()

    def log_message(self, *args):
        pass


class DevTools:
    """Minimal Chrome DevTools Protocol client over a stdlib WebSocket (text frames only)."""

    def __init__(self, ws_url):
        u = urllib.parse.urlsplit(ws_url)
        self.sock = socket.create_connection((u.hostname, u.port), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f"GET {u.path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\nUpgrade: websocket\r\n"
                           f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.sock.recv(1)
        assert b" 101 " in head.split(b"\r\n")[0], head
        self.next_id = 0

    def _recv_exact(self, n):
        data = b""
        while len(data) < n:
            chunk = self.sock.recv(n - len(data))
            if not chunk:
                raise ConnectionError("DevTools closed")
            data += chunk
        return data

    def _recv(self):
        message = b""
        while True:
            b0, b1 = self._recv_exact(2)
            n = b1 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._recv_exact(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._recv_exact(8))[0]
            message += self._recv_exact(n)
            if b0 & 0x80:
                return json.loads(message)

    def call(self, method, **params):
        self.next_id += 1
        payload = json.dumps({"id": self.next_id, "method": method, "params": params}).encode()
        mask = os.urandom(4)
        n = len(payload)
        header = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n)
                                  if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n))
        self.sock.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))
        while True:
            reply = self._recv()
            if reply.get("id") == self.next_id:
                if "error" in reply:
                    raise RuntimeError(f"{method}: {reply['error']}")
                return reply["result"]


# Ready when the detail and its history diff are drawn, nothing is loading and fonts are in.
READY = ("!!document.querySelector('.detail-col .diff') && !document.querySelector('[aria-busy=\"true\"]')"
         " && document.fonts.status === 'loaded'")


def capture(url, out):
    with tempfile.TemporaryDirectory() as profile:
        chrome = subprocess.Popen([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                                   "--remote-debugging-port=0", f"--user-data-dir={profile}", "about:blank"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            port_file = Path(profile) / "DevToolsActivePort"
            for _ in range(200):
                if port_file.exists() and port_file.read_text().strip():
                    break
                time.sleep(0.05)
            port = int(port_file.read_text().split()[0])
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list") as r:
                page = next(t for t in json.load(r) if t["type"] == "page")
            cdp = DevTools(page["webSocketDebuggerUrl"])
            cdp.call("Emulation.setDeviceMetricsOverride", width=WIDTH, height=HEIGHT, deviceScaleFactor=2, mobile=False)
            cdp.call("Page.navigate", url=url)
            deadline = time.time() + 20
            while not cdp.call("Runtime.evaluate", expression=READY)["result"].get("value"):
                if time.time() > deadline:
                    raise SystemExit(f"panel did not render: {url}")
                time.sleep(0.1)
            time.sleep(0.6)  # let entrance transitions and the live indicator settle
            shot = cdp.call("Page.captureScreenshot", format="png", captureBeyondViewport=False)
            out.write_bytes(base64.b64decode(shot["data"]))
        finally:
            chrome.terminate()
            chrome.wait(timeout=10)


def main():
    if not BINARY.exists():
        print(f"{BINARY} not found; run cargo build.")
        return 1
    if not Path(CHROME).exists():
        print(f"Chrome not found at {CHROME}; set CHROME.")
        return 1
    out_dir = ROOT / "site-src" / "screenshots"
    now = datetime.datetime.now(datetime.timezone.utc)
    for lang in LANGS:
        demo = json.loads((ROOT / "site-src" / "demo" / f"{lang}.json").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory(prefix="brainkeep-demo-") as tmp:
            server = Server(Path(tmp), demo["owner"]["handle"])
            server.start()
            try:
                stamps, focus = seed(server, demo)
            finally:
                server.stop()
            backdate(server, demo, stamps, now)
            server.start()
            proxy = None
            try:
                cookie, _ = server.login()
                proxy = Proxy(server, cookie)
                threading.Thread(target=proxy.serve_forever, daemon=True).start()
                project, memory_id = focus
                base = f"http://127.0.0.1:{proxy.server_port}"
                target = (f"{base}/panel/#/m/{urllib.parse.quote(project, safe='')}/"
                          f"{urllib.parse.quote(memory_id, safe='')}")
                for theme in THEMES:
                    out = out_dir / f"panel-{lang}-{theme}.png"
                    url = f"{base}/demo-setup?" + urllib.parse.urlencode({"lang": lang, "theme": theme, "next": target})
                    capture(url, out)
                    print("wrote", out.relative_to(ROOT))
            finally:
                if proxy:
                    proxy.shutdown()
                server.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
