"""OAuth authorization server end to end, with a simulated Google on loopback.
All identities, credentials and memories are synthetic."""
import base64
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.parse import parse_qs, urlencode, urlsplit

BINARY = Path(os.environ.get('BRAINKEEP_BINARY', 'target/debug/brainkeep')).resolve()
HOST = 'memory.example.test'
BASE = f'https://{HOST}'
STATIC = 'a' * 64
CALLBACK = 'https://claude.ai/api/mcp/auth_callback'


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b'=').decode()


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


class FakeGoogle(http.server.BaseHTTPRequestHandler):
    """Token endpoint returning an unsigned ID token built from `claims`."""
    claims = {}
    received = []

    def do_POST(self):
        body = self.rfile.read(int(self.headers['Content-Length'])).decode()
        form = {k: v[0] for k, v in parse_qs(body).items()}
        FakeGoogle.received.append(form)
        if form.get('code') != 'google-code':
            self.send_response(400); self.end_headers(); return
        token = '.'.join([b64(b'{"alg":"RS256"}'), b64(json.dumps(FakeGoogle.claims).encode()), b64(b'sig')])
        out = json.dumps({'id_token': token, 'access_token': 'ignored'}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *args):
        pass


class OAuthTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.google_port = free_port()
        cls.google = http.server.HTTPServer(('127.0.0.1', cls.google_port), FakeGoogle)
        threading.Thread(target=cls.google.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.google.shutdown()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='brain-oauth-')
        self.root = Path(self.temp.name)
        self.auth = self.root / 'auth.json'
        self.auth.write_text(json.dumps({'codex': STATIC}))
        self.auth.chmod(0o600)
        self.google_file = self.root / 'google.json'
        self.write_google({})
        self.port = free_port()
        self.logs = open(self.root / 'log', 'wb')
        self.daemon = None
        self.start()
        self.users('invite', 'ana@example.test', '--handle', 'ana')

    def tearDown(self):
        self.stop()
        self.logs.close()
        self.temp.cleanup()

    def write_google(self, extra):
        g = f'http://127.0.0.1:{self.google_port}'
        config = {'client_id': 'google-client', 'client_secret': 'google-secret',
                  'authorization_endpoint': g + '/auth', 'token_endpoint': g + '/token', **extra}
        self.google_file.write_text(json.dumps(config))
        self.google_file.chmod(0o600)

    def command(self, oauth=True):
        cmd = [str(BINARY), '--data-dir', str(self.root / 'data'), 'serve', '--http-port', str(self.port),
               '--http-host', HOST, '--auth-file', str(self.auth)]
        return cmd + (['--oauth-file', str(self.google_file)] if oauth else [])

    def start(self, oauth=True):
        self.daemon = subprocess.Popen(self.command(oauth), stdout=subprocess.DEVNULL, stderr=self.logs)
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

    def stop(self):
        if self.daemon and self.daemon.poll() is None:
            self.daemon.terminate()
            self.daemon.wait(timeout=10)

    def users(self, *args):
        out = subprocess.run([str(BINARY), '--data-dir', str(self.root / 'data'), 'users', *args],
                             capture_output=True, text=True, timeout=10)
        self.assertEqual(out.returncode, 0, out.stderr)
        return json.loads(out.stdout)

    def http(self, method, path, body=None, headers=None, token=None):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=15)
        h = {'Host': HOST, **(headers or {})}
        if token:
            h['Authorization'] = 'Bearer ' + token
        c.request(method, path, body, h)
        r = c.getresponse()
        out = (r.status, r.read(), {k.lower(): v for k, v in r.getheaders()})
        c.close()
        return out

    def mcp(self, token, name, args):
        rpc = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': name, 'arguments': args}}
        status, body, _ = self.http('POST', '/mcp', json.dumps(rpc), {
            'Accept': 'application/json, text/event-stream', 'Content-Type': 'application/json',
            'MCP-Protocol-Version': '2025-03-26'}, token)
        if status != 200:
            return status, None
        result = json.loads(body)['result']
        if result.get('isError'):
            return status, ValueError(result['content'][0]['text'])
        return status, json.loads(result['content'][0]['text'])['data']

    def register(self, uris=(CALLBACK,)):
        status, body, _ = self.http('POST', '/oauth/register', json.dumps(
            {'redirect_uris': list(uris), 'client_name': 'Claude', 'token_endpoint_auth_method': 'none'}),
            {'Content-Type': 'application/json'})
        return status, json.loads(body) if body else {}

    def token(self, form):
        status, body, headers = self.http('POST', '/oauth/token', urlencode(form),
                                          {'Content-Type': 'application/x-www-form-urlencoded'})
        self.assertEqual(headers.get('cache-control'), 'no-store')
        return status, json.loads(body)

    def login(self, client, claims=None, email='ana@example.test'):
        """Runs authorize → simulated Google → callback; returns (query at client callback, verifier)."""
        verifier = secrets.token_urlsafe(48)
        challenge = b64(hashlib.sha256(verifier.encode()).digest())
        status, _, headers = self.http('GET', '/oauth/authorize?' + urlencode({
            'response_type': 'code', 'client_id': client, 'redirect_uri': CALLBACK,
            'code_challenge': challenge, 'code_challenge_method': 'S256', 'state': 'client-state',
            'resource': BASE + '/mcp'}))
        self.assertEqual(status, 302)
        google = urlsplit(headers['location'])
        q = {k: v[0] for k, v in parse_qs(google.query).items()}
        self.assertEqual((google.path, q['client_id'], q['scope'], q['code_challenge_method']),
                         ('/auth', 'google-client', 'openid email profile', 'S256'))
        self.assertEqual(q['redirect_uri'], BASE + '/oauth/google/callback')
        FakeGoogle.claims = {'iss': 'https://accounts.google.com', 'aud': 'google-client', 'sub': 'g-' + email,
                             'email': email, 'email_verified': True, 'exp': int(time.time()) + 300,
                             'nonce': q['nonce'], **(claims or {})}
        FakeGoogle.received.clear()
        status, _, headers = self.http('GET', '/oauth/google/callback?' + urlencode(
            {'state': q['state'], 'code': 'google-code'}))
        self.assertEqual(status, 302)
        back = urlsplit(headers['location'])
        self.assertEqual(f'{back.scheme}://{back.netloc}{back.path}', CALLBACK)
        if FakeGoogle.received:
            sent = FakeGoogle.received[0]
            self.assertEqual((sent['client_secret'], sent['grant_type']), ('google-secret', 'authorization_code'))
            self.assertEqual(b64(hashlib.sha256(sent['code_verifier'].encode()).digest()), q['code_challenge'])
        return {k: v[0] for k, v in parse_qs(back.query).items()}, verifier

    def test_discovery_and_challenge(self):
        status, _, headers = self.http('POST', '/mcp')
        self.assertEqual(status, 401)
        self.assertEqual(headers['www-authenticate'],
                         f'Bearer realm="brainkeep", resource_metadata="{BASE}/.well-known/oauth-protected-resource/mcp"')
        for path in ('/.well-known/oauth-protected-resource/mcp', '/.well-known/oauth-protected-resource'):
            status, body, _ = self.http('GET', path)
            meta = json.loads(body)
            self.assertEqual((status, meta['resource'], meta['authorization_servers']), (200, BASE + '/mcp', [BASE]))
        meta = json.loads(self.http('GET', '/.well-known/oauth-authorization-server')[1])
        self.assertEqual(meta['issuer'], BASE)
        self.assertEqual(meta['code_challenge_methods_supported'], ['S256'])
        self.assertEqual(meta['registration_endpoint'], BASE + '/oauth/register')
        self.assertEqual(meta['token_endpoint_auth_methods_supported'], ['none'])

    def test_registration_rules_and_rate_limit(self):
        for bad in (['http://claude.ai/cb'], ['https://x.test/cb#f'], [], ['javascript:alert(1)'],
                    ['https://evil.test/cb'], ['https://claude.ai/evil'], ['https://claude.ai.evil.test/api/mcp/auth_callback'],
                    ['https://chatgpt.com/connector/oauth/a/b'], ['http://localhost:80@evil.test/cb'],
                    [CALLBACK, 'https://evil.test/cb']):
            self.assertEqual(self.register(bad)[0], 400, bad)
        status, body, _ = self.http('POST', '/oauth/register', json.dumps(
            {'redirect_uris': [CALLBACK], 'token_endpoint_auth_method': 'client_secret_basic'}))
        self.assertEqual(status, 400)
        status, meta = self.register()
        self.assertEqual((status, meta['token_endpoint_auth_method'], meta['redirect_uris']), (201, 'none', [CALLBACK]))
        for ok in ('https://claude.com/api/mcp/auth_callback', 'https://chatgpt.com/connector_platform_oauth_redirect',
                   'https://chatgpt.com/connector/oauth/Ab_9-x', 'http://127.0.0.1:1455/callback/XuuuHAzz'):
            self.assertEqual(self.register([ok])[0], 201, ok)
        statuses = [self.register()[0] for _ in range(20)]
        self.assertIn(429, statuses)

    def test_authorize_rejects_bad_requests_without_open_redirects(self):
        client = self.register()[1]['client_id']
        base = {'response_type': 'code', 'client_id': client, 'redirect_uri': CALLBACK,
                'code_challenge': 'a' * 43, 'code_challenge_method': 'S256', 'state': 's'}
        for change in ({'client_id': 'sbclient_unknown000000'}, {'redirect_uri': 'https://evil.test/cb'}):
            status, _, headers = self.http('GET', '/oauth/authorize?' + urlencode({**base, **change}))
            self.assertEqual((status, 'location' in headers), (400, False), change)
        for change in ({'code_challenge_method': 'plain'}, {'code_challenge': 'short'}, {'state': ''},
                       {'resource': 'https://other.test/mcp'}, {'response_type': 'token'}):
            status, _, headers = self.http('GET', '/oauth/authorize?' + urlencode({**base, **change}))
            self.assertEqual(status, 302, change)
            self.assertTrue(headers['location'].startswith(CALLBACK + '?error='), change)
        status, _, _ = self.http('GET', '/oauth/google/callback?state=forged&code=google-code')
        self.assertEqual(status, 400)

    def test_full_flow_tokens_refresh_rotation_and_disable(self):
        client = self.register()[1]['client_id']
        back, verifier = self.login(client)
        self.assertEqual((back['state'], back['iss']), ('client-state', BASE))
        status, tokens = self.token({'grant_type': 'authorization_code', 'code': back['code'], 'client_id': client,
                                     'redirect_uri': CALLBACK, 'code_verifier': verifier, 'resource': BASE + '/mcp'})
        self.assertEqual((status, tokens['token_type'], tokens['expires_in']), (200, 'Bearer', 3600))
        status, saved = self.mcp(tokens['access_token'], 'remember', {
            'project': 'demo', 'operation_id': 'o1', 'content': 'Nota sintética por OAuth',
            'source': 'synthetic:oauth', 'agent': 'claude-ai'})
        self.assertEqual((status, saved['project'], saved['principal']), (200, 'demo', 'ana'))
        # The admin's static credential does not see ana's project.
        self.assertEqual(self.mcp(STATIC, 'status', {})[1]['memories'], 0)
        # Codes are single use; replay revokes what the code produced.
        status, err = self.token({'grant_type': 'authorization_code', 'code': back['code'], 'client_id': client,
                                  'redirect_uri': CALLBACK, 'code_verifier': verifier})
        self.assertEqual((status, err['error']), (400, 'invalid_grant'))
        self.assertEqual(self.mcp(tokens['access_token'], 'status', {})[0], 401)

        back, verifier = self.login(client)
        status, t1 = self.token({'grant_type': 'authorization_code', 'code': back['code'], 'client_id': client,
                                 'redirect_uri': CALLBACK, 'code_verifier': verifier})
        self.assertEqual(status, 200)
        status, t2 = self.token({'grant_type': 'refresh_token', 'refresh_token': t1['refresh_token'], 'client_id': client})
        self.assertEqual(status, 200)
        self.assertNotEqual(t2['refresh_token'], t1['refresh_token'])
        self.assertEqual(self.mcp(t2['access_token'], 'status', {})[1]['memories'], 1)
        # Reusing a rotated refresh token revokes the whole family.
        self.assertEqual(self.token({'grant_type': 'refresh_token', 'refresh_token': t1['refresh_token'],
                                     'client_id': client})[0], 400)
        self.assertEqual(self.mcp(t2['access_token'], 'status', {})[0], 401)
        self.assertEqual(self.token({'grant_type': 'refresh_token', 'refresh_token': t2['refresh_token'],
                                     'client_id': client})[0], 400)

        # Disabling the user ends OAuth sessions immediately.
        back, verifier = self.login(client)
        _, t3 = self.token({'grant_type': 'authorization_code', 'code': back['code'], 'client_id': client,
                            'redirect_uri': CALLBACK, 'code_verifier': verifier})
        self.assertEqual(self.mcp(t3['access_token'], 'status', {})[0], 200)
        self.users('disable', 'ana')
        self.assertEqual(self.mcp(t3['access_token'], 'status', {})[0], 401)
        self.assertEqual(self.token({'grant_type': 'refresh_token', 'refresh_token': t3['refresh_token'],
                                     'client_id': client})[0], 400)
        # Re-inviting does not revive the old session.
        self.users('invite', 'ana@example.test', '--handle', 'ana')
        self.assertEqual(self.mcp(t3['access_token'], 'status', {})[0], 401)
        self.assertEqual(self.token({'grant_type': 'refresh_token', 'refresh_token': t3['refresh_token'],
                                     'client_id': client})[0], 400)

    def test_rate_limits_are_per_client_and_survive_address_churn(self):
        path = '/oauth/authorize?client_id=nope'
        ip = lambda addr: {'CF-Connecting-IP': addr}
        self.assertIn(429, [self.http('GET', path, headers=ip('203.0.113.7'))[0] for _ in range(61)])
        self.assertEqual(self.http('GET', path, headers=ip('198.51.100.9'))[0], 400)
        # One IPv6 /64 is one client, and filling the table only evicts old windows.
        self.assertIn(429, [self.http('GET', path, headers=ip(f'2001:db8:1:1::{i:x}'))[0] for i in range(61)])
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=15)
        for i in range(10070):  # past the table size, enough to exhaust a shared bucket
            c.request('GET', path, headers={'Host': HOST, **ip(f'2001:db8:{i >> 16:x}:{i & 0xffff:x}::1')})
            c.getresponse().read()
        c.close()
        self.assertEqual(self.http('GET', path, headers=ip('192.0.2.1'))[0], 400)

    def test_users_of_one_client_have_separate_receipts(self):
        # client_id is public, so two people may authorize the same client.
        self.users('invite', 'bob@example.test', '--handle', 'bob')
        client = self.register()[1]['client_id']
        tokens = {}
        for email in ('ana@example.test', 'bob@example.test'):
            back, verifier = self.login(client, email=email)
            status, tokens[email] = self.token({'grant_type': 'authorization_code', 'code': back['code'],
                                                'client_id': client, 'redirect_uri': CALLBACK, 'code_verifier': verifier})
            self.assertEqual(status, 200)
        note = {'project': 'demo', 'operation_id': 'same-op', 'content': 'Mismo texto sintético',
                'source': 'synthetic:oauth', 'agent': 'claude-ai'}
        ana = self.mcp(tokens['ana@example.test']['access_token'], 'remember', note)[1]
        bob = self.mcp(tokens['bob@example.test']['access_token'], 'remember', note)[1]
        self.assertEqual((ana['principal'], bob['principal']), ('ana', 'bob'))
        self.assertNotEqual(ana['id'], bob['id'])
        self.assertEqual(self.mcp(tokens['bob@example.test']['access_token'], 'status', {})[1]['memories'], 1)

    def test_token_endpoint_checks_pkce_client_and_redirect(self):
        client = self.register()[1]['client_id']
        other = self.register()[1]['client_id']
        for change in ({'code_verifier': secrets.token_urlsafe(48)}, {'client_id': other},
                       {'redirect_uri': 'https://claude.ai/other'}):
            back, verifier = self.login(client)
            form = {'grant_type': 'authorization_code', 'code': back['code'], 'client_id': client,
                    'redirect_uri': CALLBACK, 'code_verifier': verifier, **change}
            self.assertEqual(self.token(form)[1]['error'], 'invalid_grant', change)
        self.assertEqual(self.token({'grant_type': 'password', 'client_id': client})[1]['error'], 'unsupported_grant_type')
        status, _, _ = self.http('POST', '/oauth/token', 'grant_type=refresh_token&grant_type=refresh_token&client_id=x',
                                 {'Content-Type': 'application/x-www-form-urlencoded'})
        self.assertEqual(status, 400)

    def test_google_identity_must_be_invited_verified_and_bound(self):
        client = self.register()[1]['client_id']
        for claims, email in (({}, 'intruso@example.test'), ({'email_verified': False}, 'ana@example.test'),
                              ({'aud': 'otro-cliente'}, 'ana@example.test'), ({'nonce': 'forjado'}, 'ana@example.test'),
                              ({'exp': int(time.time()) - 10}, 'ana@example.test'),
                              ({'iss': 'https://evil.test'}, 'ana@example.test')):
            back, _ = self.login(client, claims, email)
            self.assertEqual((back.get('error'), 'code' in back), ('access_denied', False), claims or email)
        self.assertEqual([u['handle'] for u in self.users('list')['users']], ['owner', 'ana'])
        # First successful login binds ana's Google subject; another account is then rejected.
        back, _ = self.login(client)
        self.assertIn('code', back)
        back, _ = self.login(client, {'sub': 'otra-cuenta'})
        self.assertEqual(back.get('error'), 'access_denied')

    def test_oauth_is_off_without_config_and_bad_config_fails_closed(self):
        self.stop()
        self.start(oauth=False)
        status, _, headers = self.http('POST', '/mcp')
        self.assertEqual((status, headers['www-authenticate']), (401, 'Bearer realm="brainkeep"'))
        self.assertEqual(self.http('GET', '/.well-known/oauth-authorization-server')[0], 404)
        self.assertEqual(self.http('GET', '/oauth/authorize')[0], 404)
        self.stop()
        for extra, mode in (({'extra': 1}, 0o600), ({'token_endpoint': 'http://example.test/token'}, 0o600),
                            ({'client_secret': ''}, 0o600), ({}, 0o644)):
            self.write_google(extra)
            self.google_file.chmod(mode)
            out = subprocess.run(self.command(), capture_output=True, text=True, timeout=10)
            self.assertNotEqual(out.returncode, 0, (extra, mode))


if __name__ == '__main__':
    unittest.main()
