"""Listen address and client IP header for hosts without Cloudflare (Railway, Docker).

All credentials are synthetic."""
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest

BINARY = Path(os.environ.get('BRAINKEEP_BINARY', 'target/debug/brainkeep')).resolve()
AGENT = 'a' * 64
READ = 'b' * 64
KEY = 'c' * 64
HOST = 'memory.example.test'
ORIGIN = 'https://' + HOST


def external_address():
    """A non-loopback address of this host, or None without a route (no packet is sent)."""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        try:
            s.connect(('192.0.2.1', 9))
            address = s.getsockname()[0]
        except OSError:
            return None
    return None if address.startswith('127.') or address == '0.0.0.0' else address


class ListenTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='brain-listen-')
        self.root = Path(self.temp.name)
        self.auth = self.root / 'auth.json'
        self.auth.write_text(json.dumps({'agent': AGENT}))
        self.auth.chmod(0o600)
        self.panel = self.root / 'panel.json'
        self.panel.write_text(json.dumps({'read': READ, 'session_key': KEY}))
        self.panel.chmod(0o600)
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            self.port = s.getsockname()[1]
        self.logs = open(self.root / 'log', 'wb')
        self.daemon = None

    def tearDown(self):
        if self.daemon and self.daemon.poll() is None:
            self.daemon.terminate()
            self.daemon.wait(timeout=10)
        self.logs.close()
        self.temp.cleanup()

    def command(self, *extra):
        return [str(BINARY), '--data-dir', str(self.root / 'data'), 'serve', '--http-port', str(self.port),
                '--http-host', HOST, '--auth-file', str(self.auth), '--panel-auth-file', str(self.panel), *extra]

    def start(self, *extra):
        self.daemon = subprocess.Popen(self.command(*extra), stdout=subprocess.DEVNULL, stderr=self.logs)
        for _ in range(100):
            if self.daemon.poll() is not None:
                raise AssertionError('daemon failed: ' + (self.root / 'log').read_text())
            try:
                if self.login(READ, {}, '127.0.0.1') == 200:
                    return
            except OSError:
                pass
            time.sleep(.05)
        raise AssertionError('startup timeout')

    def login(self, token, headers, address='127.0.0.1'):
        c = http.client.HTTPConnection(address, self.port, timeout=5)
        c.request('POST', '/panel/api/login', json.dumps({'token': token}),
                  {'Host': HOST, 'Origin': ORIGIN, 'Content-Type': 'application/json', **headers})
        r = c.getresponse(); r.read(); c.close()
        return r.status

    def reachable(self, address):
        try:
            with socket.create_connection((address, self.port), timeout=2):
                return True
        except OSError:
            return False

    def test_default_listens_on_loopback_only(self):
        self.start()
        outside = external_address()
        if outside is None:
            self.skipTest('no non-loopback address on this host')
        self.assertFalse(self.reachable(outside))

    def test_listen_on_all_interfaces(self):
        self.start('--http-listen', '0.0.0.0')
        self.assertTrue(self.reachable('127.0.0.1'))
        outside = external_address()
        if outside is not None:
            self.assertTrue(self.reachable(outside))
            self.assertEqual(self.login(READ, {}, outside), 200)

    def test_throttle_follows_configured_header(self):
        self.start('--http-listen', '0.0.0.0', '--client-ip-header', 'X-Real-IP')
        attacker = {'X-Real-IP': '203.0.113.9'}
        statuses = [self.login('0' * 64, attacker) for _ in range(11)]
        self.assertEqual(statuses[-1], 429)
        # Another client keeps its own bucket.
        self.assertEqual(self.login(READ, {'X-Real-IP': '198.51.100.7'}), 200)
        # The Cloudflare header no longer identifies clients: rotating it does not escape.
        self.assertEqual(self.login(READ, {**attacker, 'CF-Connecting-IP': '192.0.2.44'}), 429)
        # Duplicated or malformed values fall into the shared bucket, not a fresh one.
        for spoof in ({'X-Real-IP': 'x' * 50}, {'X-Real-IP': '1.2.3.4, 5.6.7.8'}):
            self.assertEqual(self.login('0' * 64, spoof), 401)
        # An IPv4-mapped IPv6 address is the same client as its IPv4 address.
        self.assertEqual(self.login(READ, {'X-Real-IP': '::ffff:203.0.113.9'}), 429)

    def test_default_header_is_still_cloudflare(self):
        self.start()
        attacker = {'CF-Connecting-IP': '203.0.113.9'}
        self.assertEqual([self.login('0' * 64, attacker) for _ in range(11)][-1], 429)
        self.assertEqual(self.login(READ, {'CF-Connecting-IP': '198.51.100.7'}), 200)

    def test_invalid_values_refuse_start(self):
        for extra in (['--http-listen', 'localhost'], ['--http-listen', '0.0.0.0:80'],
                      ['--client-ip-header', ''], ['--client-ip-header', 'bad header'],
                      ['--client-ip-header', 'authorization'], ['--client-ip-header', 'Host'],
                      ['--client-ip-header', 'X-Forwarded-For'], ['--client-ip-header', 'forwarded']):
            with self.subTest(extra=extra):
                out = subprocess.run(self.command(*extra), capture_output=True, timeout=10)
                self.assertNotEqual(out.returncode, 0)
        # Neither flag is accepted without HTTP.
        out = subprocess.run([str(BINARY), '--data-dir', str(self.root / 'data'), 'serve',
                              '--http-listen', '0.0.0.0'], capture_output=True, timeout=10)
        self.assertNotEqual(out.returncode, 0)


if __name__ == '__main__':
    unittest.main()
