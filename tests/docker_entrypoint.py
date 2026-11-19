"""Container entrypoint (deploy/docker-entrypoint.sh) run directly, without Docker.

Runs as the current non-root user with the debug binary; all credentials are synthetic."""
import http.client
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import tempfile
import time
import unittest

ROOT_DIR = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get('BRAINKEEP_BINARY', ROOT_DIR / 'target/debug/brainkeep')).resolve()
ENTRYPOINT = ROOT_DIR / 'deploy/docker-entrypoint.sh'
SHELL = os.environ.get('ENTRYPOINT_SH', '/bin/sh')
HOST = 'memory.example.test'
ADMIN = '1' * 63 + 'a'
ADMIN2 = '2' * 63 + 'b'


class EntrypointTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='bk-entry-')
        self.root = Path(self.temp.name) / 'data'
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            self.port = s.getsockname()[1]
        self.logs = open(Path(self.temp.name) / 'log', 'wb')
        self.proc = None

    def tearDown(self):
        self.stop()
        self.logs.close()
        self.temp.cleanup()

    def env(self, **extra):
        env = {'PATH': os.environ['PATH'], 'BRAINKEEP_ROOT': str(self.root), 'BRAINKEEP_BIN': str(BINARY),
               'PORT': str(self.port), 'BRAINKEEP_LISTEN': '127.0.0.1', 'BRAINKEEP_HOST': HOST,
               'PANEL_ADMIN_KEY': ADMIN}
        env.update(extra)
        return {k: v for k, v in env.items() if v is not None}

    def run_once(self, **extra):
        return subprocess.run([SHELL, str(ENTRYPOINT)], env=self.env(**extra), capture_output=True,
                              text=True, timeout=20)

    def start(self, **extra):
        self.proc = subprocess.Popen([SHELL, str(ENTRYPOINT)], env=self.env(**extra),
                                     stdout=subprocess.DEVNULL, stderr=self.logs)
        for _ in range(200):
            if self.proc.poll() is not None:
                raise AssertionError('entrypoint exited: ' + (Path(self.temp.name) / 'log').read_text())
            try:
                self.http('GET', '/panel')
                return
            except OSError:
                time.sleep(.05)
        raise AssertionError('startup timeout')

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            self.proc.wait(timeout=10)
        self.proc = None

    def http(self, method, path, body=None, headers=None):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        c.request(method, path, body, {'Host': HOST, 'Origin': 'https://' + HOST,
                                       'Content-Type': 'application/json', **(headers or {})})
        r = c.getresponse(); data = r.read(); c.close()
        return r.status, data

    def login(self, token, headers=None):
        return self.http('POST', '/panel/api/login', json.dumps({'token': token}), headers)[0]

    def args(self):
        out = subprocess.run(['ps', '-o', 'args=', '-p', str(self.proc.pid)], capture_output=True, text=True)
        return out.stdout

    def private(self, path):
        mode = stat.S_IMODE(path.stat().st_mode)
        self.assertEqual(mode & 0o077, 0, f'{path} is {oct(mode)}')

    def test_refuses_incomplete_configuration(self):
        cases = [dict(BRAINKEEP_HOST=None), dict(PANEL_ADMIN_KEY=None), dict(PANEL_ADMIN_KEY='abc'),
                 dict(PANEL_ADMIN_KEY=ADMIN + '\n' + ADMIN),
                 dict(PANEL_ADMIN_KEY='0' * 63 + 'g'), dict(GOOGLE_CLIENT_ID='id-only')]
        for extra in cases:
            with self.subTest(extra=extra):
                out = self.run_once(**extra)
                self.assertNotEqual(out.returncode, 0)
                self.assertIn('brainkeep:', out.stderr)
                self.assertNotIn(ADMIN, out.stderr + out.stdout)

    def test_first_start_generates_private_credentials_and_restart_keeps_them(self):
        self.start()
        config = self.root / 'config'
        for name in ('auth.json', 'panel.json'):
            self.private(config / name)
        for d in (self.root, config, self.root / 'store'):
            self.private(d)
        self.assertFalse((config / 'google.json').exists())
        auth = (config / 'auth.json').read_text()
        panel = json.loads((config / 'panel.json').read_text())
        self.assertEqual(panel['admin'], ADMIN)
        self.assertEqual(len({panel['read'], panel['session_key'], ADMIN}), 3)
        self.assertEqual(self.login(ADMIN), 200)
        self.assertIn('--client-ip-header CF-Connecting-IP', self.args())
        self.assertIn('--owner admin', self.args())
        self.stop()
        # Restart with a rotated admin key: other secrets stay, the old admin key stops working.
        self.start(PANEL_ADMIN_KEY=ADMIN2)
        self.assertEqual((config / 'auth.json').read_text(), auth)
        rotated = json.loads((config / 'panel.json').read_text())
        self.assertEqual((rotated['read'], rotated['session_key'], rotated['admin']),
                         (panel['read'], panel['session_key'], ADMIN2))
        self.assertEqual(self.login(ADMIN), 401)
        self.assertEqual(self.login(ADMIN2), 200)

    def test_railway_defaults_google_and_admin_invite(self):
        self.start(BRAINKEEP_HOST=None, RAILWAY_PUBLIC_DOMAIN=HOST, RAILWAY_PROJECT_ID='synthetic-project',
                   BRAINKEEP_OWNER='ana', BRAINKEEP_ADMIN_EMAIL='ana@example.test',
                   GOOGLE_CLIENT_ID='synthetic.apps.googleusercontent.com', GOOGLE_CLIENT_SECRET='synthetic-secret')
        args = self.args()
        self.assertIn('--client-ip-header X-Real-IP', args)
        self.assertIn('--owner ana', args)
        self.assertIn('--oauth-file', args)
        self.private(self.root / 'config' / 'google.json')
        self.assertEqual(self.http('GET', '/.well-known/oauth-authorization-server')[0], 200)
        users = []
        for _ in range(100):
            out = subprocess.run([str(BINARY), '--data-dir', str(self.root / 'store'), 'users', 'list'],
                                 capture_output=True, text=True, timeout=10)
            users = json.loads(out.stdout)['users'] if out.returncode == 0 else []
            if any(u.get('email') == 'ana@example.test' for u in users):
                break
            time.sleep(.05)
        self.assertEqual([(u['handle'], u.get('email')) for u in users], [('ana', 'ana@example.test')])
        self.assertTrue((self.root / 'config' / 'admin-invited').exists())
        # The client IP header is X-Real-IP: rotating CF-Connecting-IP does not escape the limit.
        attacker = {'X-Real-IP': '203.0.113.9'}
        self.assertEqual([self.login('0' * 64, attacker) for _ in range(11)][-1], 429)
        self.assertEqual(self.login(ADMIN, {**attacker, 'CF-Connecting-IP': '192.0.2.1'}), 429)
        self.assertEqual(self.login(ADMIN, {'X-Real-IP': '198.51.100.7'}), 200)
        self.stop()
        # Removing Google removes its file; a changed admin email is reported, not applied.
        self.start(RAILWAY_PROJECT_ID='synthetic-project', BRAINKEEP_OWNER='ana',
                   BRAINKEEP_ADMIN_EMAIL='ana.new@example.test')
        self.assertFalse((self.root / 'config' / 'google.json').exists())
        self.assertEqual((self.root / 'config' / 'admin-invited').read_text(), 'ana@example.test')
        self.assertIn('stays bound to ana@example.test', (Path(self.temp.name) / 'log').read_text())
        # Secrets do not stay in the server's environment.
        env = subprocess.run(['ps', 'eww', '-o', 'command=', '-p', str(self.proc.pid)],
                             capture_output=True, text=True).stdout
        self.assertNotIn(ADMIN, env)
        self.stop()
        out = self.run_once(BRAINKEEP_OWNER='otro')
        self.assertNotEqual(out.returncode, 0)


    def test_damaged_files_and_inherited_variables(self):
        # Inherited READ_KEY/SESSION_KEY never reach panel.json.
        self.start(READ_KEY='"bad', SESSION_KEY='"bad')
        config = self.root / 'config'
        panel = json.loads((config / 'panel.json').read_text())
        self.assertNotIn('bad', panel['read'] + panel['session_key'])
        self.stop()
        # An empty auth.json (crash during the first write) is regenerated.
        (config / 'auth.json').write_text('')
        # A pretty-printed panel.json keeps its keys.
        (config / 'panel.json').write_text(json.dumps(panel, indent=2).upper().replace('"READ"', '"read"')
                                           .replace('"SESSION_KEY"', '"session_key"').replace('"ADMIN"', '"admin"'))
        self.start()
        self.assertEqual(len(json.loads((config / 'auth.json').read_text())['server']), 64)
        kept = json.loads((config / 'panel.json').read_text())
        self.assertEqual((kept['read'].lower(), kept['session_key'].lower()),
                         (panel['read'], panel['session_key']))
        self.stop()
        # A planted temporary file is replaced, not written through.
        outside = Path(self.temp.name) / 'outside'
        outside.write_text('')
        (config / 'panel.json.new').symlink_to(outside)
        self.start()
        self.stop()
        self.assertEqual(outside.read_text(), '')
        self.assertFalse((config / 'panel.json').is_symlink())
        # An unreadable panel.json stops the start instead of signing everyone out.
        (config / 'panel.json').write_text('{"read": "short"}')
        out = self.run_once()
        self.assertNotEqual(out.returncode, 0)
        self.assertIn('panel.json', out.stderr)
        self.assertEqual((config / 'panel.json').read_text(), '{"read": "short"}')


if __name__ == '__main__':
    unittest.main()
