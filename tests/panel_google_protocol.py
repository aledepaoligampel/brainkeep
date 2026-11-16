"""Panel sign-in with Google and per-user panel views, end to end with a simulated
Google on loopback. All identities, credentials and memories are synthetic."""
import hashlib
import hmac
import http.client
import http.server
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.parse import parse_qs, urlencode, urlsplit

from oauth_protocol import FakeGoogle, free_port

BINARY = Path(os.environ.get('BRAINKEEP_BINARY', 'target/debug/brainkeep')).resolve()
HOST = 'memory.example.test'
ORIGIN = 'https://' + HOST
ADMIN = 'a' * 64
ANA = 'b' * 64
READ = 'c' * 64
KEY = 'd' * 64


class PanelGoogle(unittest.TestCase):
    panel = True
    oauth = True

    @classmethod
    def setUpClass(cls):
        cls.google_port = free_port()
        cls.google = http.server.HTTPServer(('127.0.0.1', cls.google_port), FakeGoogle)
        threading.Thread(target=cls.google.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.google.shutdown()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='brain-panel-google-')
        self.root = Path(self.temp.name)
        self.write_private('auth.json', {
            'admin-codex': ADMIN,
            'ana-codex': {'token': ANA, 'user': 'ana', 'read': ['*'], 'write': ['*']}})
        self.write_private('panel.json', {'read': READ, 'session_key': KEY})
        g = f'http://127.0.0.1:{self.google_port}'
        self.write_private('google.json', {'client_id': 'google-client', 'client_secret': 'google-secret',
                                           'authorization_endpoint': g + '/auth', 'token_endpoint': g + '/token'})
        self.port = free_port()
        self.logs = open(self.root / 'log', 'wb')
        self.daemon = None
        self.start()
        self.users('invite', 'owner@example.test', '--handle', 'owner')
        self.users('invite', 'ana@example.test', '--handle', 'ana')
        self.users('invite', 'bob@example.test', '--handle', 'bob')
        self.remember(ADMIN, 'secreto', 'admin only memory')
        self.remember(ANA, 'notas', 'ana shared memory')
        self.remember(ANA, 'diario', 'ana private memory')

    def tearDown(self):
        if self.daemon and self.daemon.poll() is None:
            self.daemon.terminate()
            self.daemon.wait(timeout=10)
        self.logs.close()
        self.temp.cleanup()

    def write_private(self, name, value):
        path = self.root / name
        path.write_text(json.dumps(value))
        path.chmod(0o600)

    def start(self):
        cmd = [str(BINARY), '--data-dir', str(self.root / 'data'), 'serve', '--http-port', str(self.port),
               '--http-host', HOST, '--auth-file', str(self.root / 'auth.json')]
        if self.oauth:
            cmd += ['--oauth-file', str(self.root / 'google.json')]
        if self.panel:
            cmd += ['--panel-auth-file', str(self.root / 'panel.json')]
        self.daemon = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=self.logs)
        for _ in range(100):
            if self.daemon.poll() is not None:
                raise AssertionError('daemon failed: ' + (self.root / 'log').read_text())
            try:
                if self.http('POST', '/mcp')[0] == 401:
                    return
            except OSError:
                pass
            time.sleep(.05)
        raise AssertionError('startup timeout')

    def users(self, *args):
        out = subprocess.run([str(BINARY), '--data-dir', str(self.root / 'data'), 'users', *args],
                             capture_output=True, text=True, timeout=10)
        self.assertEqual(out.returncode, 0, out.stderr)

    def http(self, method, path, body=None, headers=None):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=15)
        c.request(method, path, body, {'Host': HOST, **(headers or {})})
        r = c.getresponse()
        out = (r.status, r.read(), r.getheaders())
        c.close()
        return out

    def tool(self, token, name, args):
        rpc = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': name, 'arguments': args}}
        status, body, _ = self.http('POST', '/mcp', json.dumps(rpc), {
            'Accept': 'application/json, text/event-stream', 'Content-Type': 'application/json',
            'MCP-Protocol-Version': '2025-03-26', 'Authorization': 'Bearer ' + token})
        self.assertEqual(status, 200)
        result = json.loads(body)['result']
        self.assertFalse(result.get('isError'), result)
        return json.loads(result['content'][0]['text'])['data']

    def remember(self, token, project, content):
        self.tool(token, 'remember', {'project': project, 'content': content, 'kind': 'note',
                                      'title': content, 'source': 'test', 'agent': 'test',
                                      'operation_id': secrets.token_hex(8)})

    @staticmethod
    def set_cookies(headers):
        return [v for k, v in headers if k.lower() == 'set-cookie']

    def start_login(self):
        status, _, headers = self.http('GET', '/oauth/panel/start')
        self.assertEqual(status, 302)
        location = dict((k.lower(), v) for k, v in headers)['location']
        q = {k: v[0] for k, v in parse_qs(urlsplit(location).query).items()}
        self.assertEqual(q['redirect_uri'], ORIGIN + '/oauth/google/callback')
        [binding] = self.set_cookies(headers)
        self.assertTrue(binding.startswith('__Host-brainkeep_panel_login='))
        self.assertIn('Path=/;', binding)
        self.assertNotIn('Domain', binding)
        for flag in ('HttpOnly', 'Secure', 'SameSite=Lax'):
            self.assertIn(flag, binding)
        return q, binding.split(';')[0]

    def finish(self, q, cookie, email, claims=None):
        FakeGoogle.claims = {'iss': 'https://accounts.google.com', 'aud': 'google-client', 'sub': 'g-' + email,
                             'email': email, 'email_verified': True, 'exp': int(time.time()) + 300,
                             'nonce': q['nonce'], **(claims or {})}
        headers = {'Cookie': cookie} if cookie else {}
        return self.http('GET', '/oauth/google/callback?' + urlencode({'state': q['state'], 'code': 'google-code'}),
                         headers=headers)

    def google_session(self, email):
        """Signs in through Google and returns the panel session cookie."""
        q, binding = self.start_login()
        status, body, headers = self.finish(q, binding, email)
        self.assertEqual(status, 200, body)
        self.assertIn(b'url=/panel/', body)
        [session] = [c for c in self.set_cookies(headers) if c.startswith('brainkeep_panel=')]
        for flag in ('HttpOnly', 'Secure', 'SameSite=Strict', 'Path=/panel'):
            self.assertIn(flag, session)
        return session.split(';')[0]

    def read_key_session(self):
        status, _, headers = self.http('POST', '/panel/api/login', json.dumps({'token': READ}),
                                       {'Origin': ORIGIN, 'Content-Type': 'application/json'})
        self.assertEqual(status, 200)
        return self.set_cookies(headers)[0].split(';')[0]

    def get(self, cookie, view, **params):
        status, body, _ = self.http('GET', f'/panel/api/{view}' + ('?' + urlencode(params) if params else ''),
                                    headers={'Cookie': cookie})
        return status, json.loads(body)

    def view(self, cookie):
        """Everything a session can see, across every panel view."""
        projects = sorted(p['project'] for p in self.get(cookie, 'projects')[1]['items'])
        listed = self.get(cookie, 'memories')[1]
        found = self.get(cookie, 'memories', q='memory')[1]
        facets = self.get(cookie, 'facets')[1]
        status = self.get(cookie, 'status')[1]
        return {'projects': projects,
                'listed': sorted(m['content'] for m in listed['items']),
                'searched': sorted(m['content'] for m in found['items']),
                'facets': sorted(facets['projects']),
                'status': (status['memories'], status['projects'])}

    def test_login_offer_and_invited_user_sees_own_and_shared_only(self):
        self.assertEqual(self.get('', 'login'), (200, {'google': True}))
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'read'})
        bob = self.google_session('bob@example.test')
        status, session = self.get(bob, 'session')
        self.assertEqual((status, session['user']), (200, 'bob'))
        self.assertEqual(self.view(bob), {
            'projects': ['ana/notas'], 'listed': ['ana shared memory'], 'searched': ['ana shared memory'],
            'facets': ['ana/notas'], 'status': (1, 1)})
        self.assertEqual(self.get(bob, 'memories', project='ana/notas')[1]['total'], 1)
        # Unshared projects of others answer exactly like missing ones.
        ana = self.google_session('ana@example.test')
        ids = {m['project']: m['id'] for m in self.get(ana, 'memories')[1]['items']}
        missing = self.get(bob, 'memory', project='ana/no-such', id='x')
        self.assertEqual(missing[0], 404)
        self.assertEqual(self.get(bob, 'memory', project='ana/diario', id=ids['diario']), missing)
        self.assertEqual(self.get(bob, 'memory', project='diario', id=ids['diario']), missing)
        self.assertEqual(self.get(bob, 'memories', project='ana/diario')[1]['total'], 0)
        self.assertEqual(self.get(bob, 'facets', project='ana/diario')[1]['projects'], [])
        self.assertEqual(self.get(bob, 'memory', project='ana/notas', id=ids['notas'])[0], 200)
        # Ana sees her projects by short name and nothing of the admin.
        self.assertEqual(self.view(ana), {
            'projects': ['diario', 'notas'], 'listed': ['ana private memory', 'ana shared memory'],
            'searched': ['ana private memory', 'ana shared memory'], 'facets': ['diario', 'notas'],
            'status': (2, 2)})
        # Revoking the share hides it on the next request.
        self.tool(ANA, 'unshare', {'project': 'notas', 'email': 'bob@example.test'})
        self.assertEqual(self.view(bob), {'projects': [], 'listed': [], 'searched': [], 'facets': [],
                                          'status': (0, 0)})
        self.assertEqual(self.get(bob, 'memory', project='ana/notas', id=ids['notas']), missing)

    def login_with(self, email, claims):
        q, binding = self.start_login()
        self.assertEqual(q['scope'], 'openid email profile')
        status, body, headers = self.finish(q, binding, email, claims)
        self.assertEqual(status, 200, body)
        [session] = [c for c in self.set_cookies(headers) if c.startswith('brainkeep_panel=')]
        return session.split(';')[0]

    def test_google_picture_and_name_are_validated_and_kept(self):
        photo = 'https://lh3.googleusercontent.com/a/ACg8ocSynthetic=s96-c'
        self.assertEqual(self.get(self.login_with('ana@example.test', {'name': 'ñ' * 128}), 'session')[1]['name'], 'ñ' * 128)
        ana = self.login_with('ana@example.test', {'picture': photo, 'name': '  Ana Sintética  '})
        self.assertEqual(self.get(ana, 'session')[1]['picture'], photo)
        self.assertEqual(self.get(ana, 'session')[1]['name'], 'Ana Sintética')
        self.assertEqual(self.get(ana, 'session')[1]['email'], 'ana@example.test')
        for bad in ('', '   ', 'a\nb', 'a\x00b', 'x' * 129, 7, 'Ana\u202egnp', '\u200bAna', 'A\u2066na'):
            ana = self.login_with('ana@example.test', {'name': bad})
            self.assertIsNone(self.get(ana, 'session')[1]['name'], bad)
        # Another user's session never carries Ana's photo; the read key shows none.
        bob = self.login_with('bob@example.test', {})
        self.assertEqual((self.get(bob, 'session')[1]['picture'], self.get(bob, 'session')[1]['name']), (None, None))
        key = self.get(self.read_key_session(), 'session')[1]
        self.assertEqual((key['picture'], key['name'], key['email']), (None, None, None))
        self.assertEqual(self.get(bob, 'session')[1]['email'], 'bob@example.test')
        for bad in ('http://lh3.googleusercontent.com/a/x', 'https://evil.test/a.png',
                    'https://lh3.googleusercontent.com.evil.test/a', 'https://lh3.googleusercontent.com/a b',
                    'https://lh3.googleusercontent.com/a"onerror=x', 'https://lh3.googleusercontent.com/<x>',
                    'https://lh3.googleusercontent.com/' + 'a' * 2048, 42, ['x']):
            ana = self.login_with('ana@example.test', {'picture': photo})
            self.assertEqual(self.get(ana, 'session')[1]['picture'], photo)
            ana = self.login_with('ana@example.test', {'picture': bad})
            self.assertIsNone(self.get(ana, 'session')[1]['picture'], bad)
        # A later login without a photo clears the stored one.
        self.login_with('ana@example.test', {'picture': photo})
        ana = self.login_with('ana@example.test', {})
        self.assertIsNone(self.get(ana, 'session')[1]['picture'])

    def test_read_key_acts_as_admin_and_google_admin_sees_the_same(self):
        key = self.read_key_session()
        status, session = self.get(key, 'session')
        self.assertEqual((status, session['user']), (200, None))
        admin_view = self.view(key)
        self.assertEqual(admin_view['projects'], ['secreto'])
        self.assertEqual(admin_view['listed'], ['admin only memory'])
        self.assertEqual(self.view(self.google_session('owner@example.test')), admin_view)

    def test_disabling_a_user_ends_their_panel_session(self):
        bob = self.google_session('bob@example.test')
        self.assertEqual(self.get(bob, 'status')[0], 200)
        self.users('disable', 'bob')
        for view in ('session', 'status', 'projects', 'facets', 'memories'):
            self.assertEqual(self.get(bob, view)[0], 401, view)
        # Re-inviting does not revive old sessions; a new sign-in works.
        self.users('invite', 'bob@example.test', '--handle', 'bob')
        self.assertEqual(self.get(bob, 'status')[0], 401)
        self.assertEqual(self.get(self.google_session('bob@example.test'), 'status')[0], 200)

    def test_logout_ends_the_session_on_the_server(self):
        bob = self.google_session('bob@example.test')
        other = self.google_session('bob@example.test')
        status, _, _ = self.http('POST', '/panel/api/logout', '{}', {
            'Origin': ORIGIN, 'Content-Type': 'application/json', 'Cookie': bob})
        self.assertEqual(status, 200)
        self.assertEqual(self.get(bob, 'status')[0], 401)
        self.assertEqual(self.get(other, 'status')[0], 200)

    def test_listing_snapshot_ignores_writes_the_session_cannot_see(self):
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'read'})
        bob = self.google_session('bob@example.test')
        first = self.get(bob, 'memories')[1]['snapshot']
        for i in range(3):
            self.remember(ANA, 'diario', f'private {i}')
            self.remember(ADMIN, 'secreto', f'admin {i}')
        self.assertEqual(self.get(bob, 'memories')[1]['snapshot'], first)
        self.remember(ANA, 'notas', 'shared again')
        second = self.get(bob, 'memories')[1]['snapshot']
        self.assertNotEqual(second, first)
        self.assertLess(second, 2 ** 53)
        # Paging with a stale snapshot is refused; with the current one it works.
        self.assertEqual(self.get(bob, 'memories', offset=1, snapshot=first)[0], 409)
        self.assertEqual(self.get(bob, 'memories', offset=1, snapshot=second)[0], 200)
        # Losing the share also changes it.
        self.tool(ANA, 'unshare', {'project': 'notas', 'email': 'bob@example.test'})
        self.assertNotEqual(self.get(bob, 'memories')[1]['snapshot'], second)

    def test_callbacks_without_this_browser_or_an_invited_email_create_no_session(self):
        cases = [
            ('missing binding cookie', lambda q, c: (None, 'bob@example.test', None)),
            ('another browser', lambda q, c: ('__Host-brainkeep_panel_login=' + 'x' * 43, 'bob@example.test', None)),
            ('duplicated binding', lambda q, c: (c + '; ' + c, 'bob@example.test', None)),
            ('not invited', lambda q, c: (c, 'eve@example.test', None)),
            ('unverified', lambda q, c: (c, 'bob@example.test', {'email_verified': False})),
            ('wrong nonce', lambda q, c: (c, 'bob@example.test', {'nonce': 'other'})),
        ]
        for name, case in cases:
            q, binding = self.start_login()
            cookie, email, claims = case(q, binding)
            status, _, headers = self.finish(q, cookie, email, claims)
            self.assertEqual(status, 403, name)
            self.assertFalse([c for c in self.set_cookies(headers) if c.startswith('brainkeep_panel=')], name)
            # The state is single use: retrying it with the right cookie fails too.
            status, _, headers = self.finish(q, binding, 'bob@example.test')
            self.assertEqual(status, 400, name)
            self.assertFalse([c for c in self.set_cookies(headers) if c.startswith('brainkeep_panel=')], name)
        # A state from someone else's login attempt (attacker-started) with a victim cookie.
        q_attacker, _ = self.start_login()
        _, victim = self.start_login()
        status, _, _ = self.finish(q_attacker, victim, 'bob@example.test')
        self.assertEqual(status, 403)
        # Google cancellation.
        q, binding = self.start_login()
        status, _, _ = self.http('GET', '/oauth/google/callback?' + urlencode(
            {'state': q['state'], 'error': 'access_denied'}), headers={'Cookie': binding})
        self.assertEqual(status, 403)

    def test_session_cookie_binds_the_user(self):
        bob = self.google_session('bob@example.test')
        value = bob.split('=', 1)[1]
        signed, mac = value.rsplit('.', 1)
        self.assertTrue(signed.startswith('v2.') and signed.endswith('.bob'))
        forged = 'brainkeep_panel=' + signed[:-3] + 'ana.' + mac
        self.assertEqual(self.get(forged, 'status')[0], 401)
        # Correctly signed v2 cookie for a handle that does not exist.
        mac_key = hmac.new(bytes.fromhex(KEY), READ.encode(), hashlib.sha256).digest()
        ghost = f'v2.{int(time.time()) + 3600}.{"0" * 32}.ghost'
        cookie = 'brainkeep_panel=' + ghost + '.' + hmac.new(mac_key, ghost.encode(), hashlib.sha256).hexdigest()
        self.assertEqual(self.get(cookie, 'status')[0], 401)
        self.assertEqual(self.get(cookie, 'session')[0], 401)


class WithoutPanel(PanelGoogle):
    panel = False

    def test_login_offer_and_invited_user_sees_own_and_shared_only(self):
        self.assertEqual(self.http('GET', '/oauth/panel/start')[0], 404)

    test_read_key_acts_as_admin_and_google_admin_sees_the_same = None
    test_disabling_a_user_ends_their_panel_session = None
    test_callbacks_without_this_browser_or_an_invited_email_create_no_session = None
    test_session_cookie_binds_the_user = None
    test_logout_ends_the_session_on_the_server = None
    test_listing_snapshot_ignores_writes_the_session_cannot_see = None
    test_google_picture_and_name_are_validated_and_kept = None


if __name__ == '__main__':
    unittest.main()
