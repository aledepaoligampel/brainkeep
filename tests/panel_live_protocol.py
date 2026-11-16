"""Live panel updates (SSE): events only for visible projects, end with the session.
Reuses the simulated-Google harness; all identities and memories are synthetic."""
import hashlib
import hmac
import select
import socket
import threading
import time
import unittest
import uuid

import panel_google_protocol as base

ANA = base.ANA


class Stream:
    """Minimal Server-Sent Events client on a raw socket (chunked transfer decoding)."""

    def __init__(self, test, cookie):
        self.sock = socket.create_connection(('127.0.0.1', test.port), timeout=10)
        self.sock.sendall((f'GET /panel/api/events HTTP/1.1\r\nHost: {base.HOST}\r\nCookie: {cookie}\r\n'
                           'Accept: text/event-stream\r\n\r\n').encode())
        self.raw = b''
        self.body = b''
        self.closed = False
        self.done = False
        deadline = time.monotonic() + 10
        while b'\r\n\r\n' not in self.raw:
            if not self._fill(deadline):
                raise AssertionError('no response head')
        head, self.raw = self.raw.split(b'\r\n\r\n', 1)
        lines = head.decode().split('\r\n')
        self.status = int(lines[0].split()[1])
        self.headers = {k.strip().lower(): v.strip() for k, v in (l.split(':', 1) for l in lines[1:])}
        self.chunked = 'chunked' in self.headers.get('transfer-encoding', '')

    def _fill(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([self.sock], [], [], remaining)[0]:
            return False
        data = self.sock.recv(65536)
        if not data:
            self.closed = True
            return False
        self.raw += data
        return True

    def _decode(self):
        while self.chunked and not self.done:
            line, sep, rest = self.raw.partition(b'\r\n')
            if not sep:
                return
            size = int(line, 16)
            if size == 0:
                self.done = True
                return
            if len(rest) < size + 2:
                return
            self.body += rest[:size]
            self.raw = rest[size + 2:]

    def event(self, timeout):
        """Next named event, None on timeout, 'EOF' when the server ended the stream."""
        deadline = time.monotonic() + timeout
        while True:
            self._decode()
            while b'\n\n' in self.body:
                block, self.body = self.body.split(b'\n\n', 1)
                for line in block.decode().split('\n'):
                    if line.startswith('event:'):
                        return line[6:].strip()
            if self.done or self.closed:
                return 'EOF'
            if not self._fill(deadline):
                if self.closed:
                    continue
                return None

    def close(self):
        self.sock.close()


class Live(base.PanelGoogle):
    def stream(self, cookie):
        s = Stream(self, cookie)
        self.addCleanup(s.close)
        return s

    def test_events_name_only_visible_changes(self):
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'read'})
        bob = self.stream(self.google_session('bob@example.test'))
        self.assertEqual(bob.status, 200)
        self.assertTrue(bob.headers['content-type'].startswith('text/event-stream'))
        self.assertIn("default-src 'none'", bob.headers['content-security-policy'])
        self.assertEqual(bob.headers['cache-control'], 'no-store')
        # A write bob cannot see sends nothing; one in the shared project does.
        self.remember(ANA, 'diario', 'private again')
        self.assertIsNone(bob.event(1.5))
        self.remember(ANA, 'notas', 'shared again')
        self.assertEqual(bob.event(5), 'change')
        # The owner's own stream sees both.
        ana = self.stream(self.google_session('ana@example.test'))
        self.remember(ANA, 'diario', 'third')
        self.assertEqual(ana.event(5), 'change')
        self.assertIsNone(bob.event(1.5))
        # Revoking the share changes bob's own view (one reload), then nothing more.
        self.tool(ANA, 'unshare', {'project': 'notas', 'email': 'bob@example.test'})
        self.assertEqual(bob.event(5), 'change')
        self.remember(ANA, 'notas', 'after revoke')
        self.assertIsNone(bob.event(1.5))

    def test_admin_key_stream_sees_only_admin_projects(self):
        key = self.stream(self.read_key_session())
        self.assertEqual(key.status, 200)
        self.remember(ANA, 'notas', 'not for the admin')
        self.assertIsNone(key.event(1.5))
        self.remember(base.ADMIN, 'secreto', 'admin write')
        self.assertEqual(key.event(5), 'change')

    def test_disabled_user_stream_ends_and_sessions_are_required(self):
        self.assertEqual(self.stream('').status, 401)
        self.assertEqual(self.stream('brainkeep_panel=v2.1.x.bob').status, 401)
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'read'})
        bob = self.stream(self.google_session('bob@example.test'))
        self.users('disable', 'bob')
        self.remember(ANA, 'notas', 'wakes the stream')
        self.assertEqual(bob.event(5), 'EOF')
        self.assertEqual(self.stream('').status, 401)

    def test_streams_per_user_are_limited(self):
        bob = self.google_session('bob@example.test')
        streams = [self.stream(bob) for _ in range(4)]
        self.assertEqual([s.status for s in streams], [200] * 4)
        extra = self.stream(bob)
        self.assertEqual(extra.status, 429)
        streams[0].close()
        time.sleep(.5)
        # A closed stream frees its slot once the server notices the disconnect.
        self.remember(ANA, 'notas', 'any write wakes idle streams')
        for _ in range(20):
            again = self.stream(bob)
            if again.status == 200:
                break
            time.sleep(.2)
        self.assertEqual(again.status, 200)

    def test_stream_ends_when_its_cookie_expires(self):
        # A read-key (v1) cookie signed like the server does, valid for 2 more seconds.
        mac_key = hmac.new(bytes.fromhex(base.KEY), base.READ.encode(), hashlib.sha256).digest()
        value = f'v1.{int(time.time()) + 2}.{uuid.uuid4().hex}'
        mac = hmac.new(mac_key, value.encode(), hashlib.sha256).hexdigest()
        stream = self.stream(f'brainkeep_panel={value}.{mac}')
        self.assertEqual(stream.status, 200)
        self.assertEqual(stream.event(6), 'EOF')

    def test_global_stream_limit_is_429(self):
        cookies = [self.read_key_session()]
        for n in range(8):
            self.users('invite', f'user{n}@example.test', '--handle', f'user{n}')
            cookies.append(self.google_session(f'user{n}@example.test'))
        streams = [self.stream(c) for c in cookies[1:] for _ in range(4)]
        self.assertEqual([s.status for s in streams], [200] * 32)
        self.assertEqual(self.stream(cookies[0]).status, 429)

    def test_logout_ends_the_stream(self):
        bob = self.google_session('bob@example.test')
        stream = self.stream(bob)
        status, _, _ = self.http('POST', '/panel/api/logout', '{}', {
            'Origin': base.ORIGIN, 'Content-Type': 'application/json', 'Cookie': bob})
        self.assertEqual(status, 200)
        self.remember(ANA, 'diario', 'wakes every stream')
        self.assertEqual(stream.event(5), 'EOF')
        self.assertEqual(self.stream(bob).status, 401)

    def test_bursts_elsewhere_reveal_nothing_and_api_stays_responsive(self):
        bob = self.stream(self.google_session('bob@example.test'))
        key = self.stream(self.read_key_session())
        others = [self.stream(self.google_session('ana@example.test')) for _ in range(4)]
        errors = []

        def write(n):
            for i in range(30):
                try:
                    self.remember(ANA, 'diario', f'burst {n} {i}')
                except AssertionError as error:  # MCP concurrency limits (503/429) are not under test
                    errors.append(error)
        threads = [threading.Thread(target=write, args=(n,)) for n in range(20)]
        for t in threads:
            t.start()
        started = time.monotonic()
        self.assertEqual(self.get(self.google_session('owner@example.test'), 'memories')[0], 200)
        self.assertLess(time.monotonic() - started, 5)
        for t in threads:
            t.join()
        # One credential gets 4 concurrent requests; the burst still lands >= 150 writes.
        self.assertLess(len(errors), 450)
        self.assertIsNone(bob.event(2))
        self.assertIsNone(key.event(.5))
        self.assertEqual(others[0].event(5), 'change')

    def test_shutdown_does_not_wait_for_open_streams(self):
        bob = self.stream(self.google_session('bob@example.test'))
        self.assertEqual(bob.status, 200)
        started = time.monotonic()
        self.daemon.terminate()
        self.daemon.wait(timeout=10)
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(bob.event(5), 'EOF')


# Run only the live tests here; the inherited ones run in panel_google_protocol.py.
for _name in dir(base.PanelGoogle):
    if _name.startswith('test_'):
        setattr(Live, _name, None)


if __name__ == '__main__':
    unittest.main()
