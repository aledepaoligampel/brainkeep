"""Read-only web panel boundary tests (S1-S7 of panel-consulta). Synthetic data only."""
import datetime
import gzip
import hashlib
import hmac
import http.client
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.parse

BINARY = Path(os.environ.get('BRAINKEEP_BINARY', 'target/debug/brainkeep')).resolve()
HOST = 'memory.example.test'
ORIGIN = 'https://' + HOST
AGENT = 'a' * 64
READ = 'c' * 64
KEY = 'd' * 64


def forge(expires, read=READ, key=KEY, nonce='0' * 32):
    mac_key = hmac.new(bytes.fromhex(key), read.encode(), hashlib.sha256).digest()
    value = f'v1.{expires}.{nonce}'
    return 'brainkeep_panel=' + value + '.' + hmac.new(mac_key, value.encode(), hashlib.sha256).hexdigest()


class Panel(unittest.TestCase):
    panel = True

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='brain-panel-')
        self.root = Path(self.temp.name)
        self.auth = self.root / 'auth.json'
        self.auth.write_text(json.dumps({'agent': AGENT}))
        self.auth.chmod(0o600)
        self.panel_auth = self.root / 'panel.json'
        self.write_panel(READ, KEY)
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            self.port = s.getsockname()[1]
        self.logs = open(self.root / 'log', 'wb')
        self.start()

    def write_panel(self, read, key):
        if self.panel_auth.exists():
            self.panel_auth.chmod(0o600)
        self.panel_auth.write_text(json.dumps({'read': read, 'session_key': key}))
        self.panel_auth.chmod(0o600)

    def start(self):
        command = [str(BINARY), '--data-dir', str(self.root / 'data'), 'serve', '--http-port', str(self.port),
                   '--http-host', HOST, '--auth-file', str(self.auth)]
        if self.panel:
            command += ['--panel-auth-file', str(self.panel_auth)]
        self.daemon = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=self.logs)
        for _ in range(100):
            if self.daemon.poll() is not None:
                raise AssertionError('daemon failed: ' + (self.root / 'log').read_text())
            try:
                if self.mcp_status() == 200:
                    return
            except OSError:
                pass
            time.sleep(.05)
        raise AssertionError('startup timeout')

    def restart(self):
        self.daemon.terminate()
        self.daemon.wait(timeout=10)
        self.start()

    def tearDown(self):
        if self.daemon.poll() is None:
            self.daemon.terminate()
            self.daemon.wait(timeout=10)
        self.logs.close()
        self.temp.cleanup()

    def http(self, method, path, body=None, cookie=None, origin=ORIGIN, ctype='application/json',
             host=HOST, bearer=None):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        headers = {'Host': host}
        if origin is not None:
            headers['Origin'] = origin
        if body is not None:
            headers['Content-Type'] = ctype
        if cookie:
            headers['Cookie'] = cookie
        if bearer:
            headers['Authorization'] = 'Bearer ' + bearer
        c.request(method, path, body, headers)
        r = c.getresponse()
        out = (r.status, r.read(), {k.lower(): v for k, v in r.getheaders()}, r.headers.get_all('Set-Cookie') or [])
        c.close()
        return out

    def mcp(self, rpc, bearer=AGENT, cookie=None):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        headers = {'Host': HOST, 'Accept': 'application/json, text/event-stream',
                   'Content-Type': 'application/json', 'MCP-Protocol-Version': '2025-03-26'}
        if bearer:
            headers['Authorization'] = 'Bearer ' + bearer
        if cookie:
            headers['Cookie'] = cookie
        c.request('POST', '/mcp', json.dumps(rpc), headers)
        r = c.getresponse()
        out = (r.status, r.read())
        c.close()
        return out

    def mcp_status(self, bearer=AGENT, cookie=None):
        return self.mcp({'jsonrpc': '2.0', 'id': 1, 'method': 'tools/list'}, bearer, cookie)[0]

    def tool(self, name, args):
        status, body = self.mcp({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call',
                                 'params': {'name': name, 'arguments': args}})
        self.assertEqual(status, 200, body)
        result = json.loads(body)['result']
        if result.get('isError'):
            raise ValueError(result['content'][0]['text'])
        return json.loads(result['content'][0]['text'])['data']

    def login(self, token=READ, **kw):
        return self.http('POST', '/panel/api/login', json.dumps({'token': token}), **kw)

    def cookie(self):
        status, body, _, cookies = self.login()
        self.assertEqual(status, 200, body)
        return cookies[0].split(';')[0]

    def get(self, path, cookie, **params):
        query = urllib.parse.urlencode(params)
        status, body, headers, _ = self.http('GET', path + ('?' + query if query else ''), cookie=cookie)
        return status, (json.loads(body) if headers.get('content-type', '').startswith('application/json') else body), headers


class Access(Panel):
    def test_login_sets_strict_session_cookie(self):
        for token in ('', 'x', AGENT, 'c' * 63, READ + 'c'):
            status, body, _, cookies = self.login(token)
            self.assertEqual(status, 401, token)
            self.assertEqual(cookies, [])
        self.assertEqual(self.login(origin='https://evil.example')[0], 403)
        self.assertEqual(self.login(origin=None)[0], 403)
        self.assertEqual(self.login(host='evil.example')[0], 403)
        self.assertEqual(self.http('POST', '/panel/api/login', 'token=' + READ,
                                   ctype='application/x-www-form-urlencoded')[0], 415)
        status, body, headers, cookies = self.login()
        self.assertEqual(status, 200, body)
        self.assertEqual(len(cookies), 1)
        attributes = [part.strip() for part in cookies[0].split(';')]
        for required in ('HttpOnly', 'Secure', 'SameSite=Strict', 'Path=/panel', 'Max-Age=2592000'):
            self.assertIn(required, attributes)
        self.assertNotIn(READ.encode(), body)
        self.assertNotIn(READ, json.dumps(headers))
        self.assertEqual(headers.get('cache-control'), 'no-store')
        self.assertNotIn(READ.encode(), (self.root / 'log').read_bytes())

    def test_session_required_and_tamper_rejected(self):
        cookie = self.cookie()
        status, body, _ = self.get('/panel/api/session', cookie)
        self.assertEqual(status, 200)
        self.assertGreater(body['expires_at'], int(time.time()) + 29 * 86400)
        now = int(time.time())
        bad = [None, 'brainkeep_panel=', cookie[:-1] + ('0' if cookie[-1] != '0' else '1'),
               forge(now - 1), forge(now + 3600, key='e' * 64), forge(now + 3600, read='f' * 64),
               forge(now + 40 * 86400), 'brainkeep_panel=v2' + cookie[len('brainkeep_panel=v1'):]]
        for path in ('/panel/api/session', '/panel/api/status', '/panel/api/projects', '/panel/api/memories',
                     '/panel/api/facets', '/panel/api/memory?project=p&id=x'):
            for value in bad:
                status, body, headers, _ = self.http('GET', path, cookie=value)
                self.assertEqual(status, 401, (path, value))
                self.assertEqual(headers.get('cache-control'), 'no-store')
        self.assertEqual(self.get('/panel/api/session', forge(now + 3600))[0], 200)
        self.assertEqual(self.get('/panel/api/session', cookie)[0], 200)
        status, _, _, _ = self.http('GET', '/panel/api/session', cookie=cookie, host='evil.example')
        self.assertEqual(status, 403)

    def test_credentials_are_not_interchangeable(self):
        cookie = self.cookie()
        self.assertEqual(self.mcp_status(bearer=None, cookie=cookie), 401)
        self.assertEqual(self.mcp_status(bearer=READ), 401)
        self.assertEqual(self.http('GET', '/panel/api/session', bearer=AGENT)[0], 401)
        self.assertEqual(self.mcp_status(), 200)

    def test_failed_logins_are_throttled(self):
        statuses = [self.login('0' * 64)[0] for _ in range(12)]
        self.assertEqual(statuses[:10], [401] * 10)
        self.assertEqual(statuses[10:], [429, 429])
        self.assertEqual(self.http('POST', '/panel/api/login', json.dumps({'token': READ}))[2].get('retry-after'), '60')
        self.assertEqual(self.login()[0], 429)

    def test_throttle_is_per_client_and_body_reads_time_out(self):
        attacker = {'CF-Connecting-IP': '203.0.113.9'}
        statuses = [self.login_from('0' * 64, attacker) for _ in range(11)]
        self.assertEqual(statuses[-1], 429)
        self.assertEqual(self.login_from(READ, {'CF-Connecting-IP': '198.51.100.7'}), 200)
        self.assertEqual(self.login_from(READ, attacker), 429)
        for spoof in ({'CF-Connecting-IP': 'x' * 50}, {'CF-Connecting-IP': '1.2.3.4, 5.6.7.8'}):
            self.assertEqual(self.login_from('0' * 64, spoof), 401)
        s = socket.create_connection(('127.0.0.1', self.port), timeout=10)
        s.sendall(('POST /panel/api/login HTTP/1.1\r\nHost: %s\r\nOrigin: %s\r\nContent-Type: application/json\r\n'
                   'Content-Length: 500\r\n\r\n{"tok' % (HOST, ORIGIN)).encode())
        started = time.time()
        reply = s.recv(4096).decode(errors='replace')
        s.close()
        self.assertIn(' 408 ', reply.split('\r\n')[0])
        self.assertLess(time.time() - started, 8)
        cookie = self.cookie()
        slow = []
        for n in range(24):
            c = socket.create_connection(('127.0.0.1', self.port), timeout=10)
            c.sendall(('POST /panel/api/login HTTP/1.1\r\nHost: %s\r\nOrigin: %s\r\nContent-Type: application/json\r\n'
                       'CF-Connecting-IP: 192.0.2.%d\r\nContent-Length: 500\r\n\r\n{' % (HOST, ORIGIN, n + 1)).encode())
            slow.append(c)
        time.sleep(.3)
        try:
            self.assertEqual(self.get('/panel/api/session', cookie)[0], 200)
            self.assertEqual(self.http('GET', '/panel', origin=None)[0], 200)
        finally:
            for c in slow:
                c.close()
        invalid = [self.login_from('0' * 64, {'CF-Connecting-IP': '999.1.1.1'}) for _ in range(11)]
        self.assertEqual(invalid[-1], 429)
        self.assertEqual(self.login_from('0' * 64, {}), 429)
        self.assertEqual(self.login_from(READ, {'CF-Connecting-IP': '2001:db8::1'}), 200)

    def login_from(self, token, headers):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        c.request('POST', '/panel/api/login', json.dumps({'token': token}),
                  {'Host': HOST, 'Origin': ORIGIN, 'Content-Type': 'application/json', **headers})
        r = c.getresponse(); r.read(); c.close()
        return r.status

    def test_logout_clears_cookie_and_key_rotation_revokes(self):
        cookie = self.cookie()
        status, _, _, cookies = self.http('POST', '/panel/api/logout', '{}', cookie=cookie)
        self.assertEqual(status, 200)
        self.assertTrue(any('Max-Age=0' in c for c in cookies))
        self.assertEqual(self.http('POST', '/panel/api/logout', '{}', cookie=cookie, origin='https://evil.example')[0], 403)
        cookie = self.cookie()
        self.write_panel(READ, 'e' * 64)
        self.restart()
        self.assertEqual(self.get('/panel/api/session', cookie)[0], 401)
        self.write_panel('b' * 64, 'e' * 64)
        self.restart()
        self.assertEqual(self.login()[0], 401)
        self.assertEqual(self.login('b' * 64)[0], 200)

    def test_invalid_panel_credentials_file_refuses_start(self):
        self.daemon.terminate()
        self.daemon.wait(timeout=10)
        for content in ({'read': AGENT, 'session_key': KEY}, {'read': READ}, {'read': READ, 'session_key': READ},
                        {'read': READ, 'session_key': '0' * 64},
                        {'read': 'x' * 64, 'session_key': KEY}):
            self.panel_auth.chmod(0o600)
            self.panel_auth.write_text(json.dumps(content))
            p = subprocess.run([str(BINARY), '--data-dir', str(self.root / 'data'), 'serve', '--http-port',
                                str(self.port), '--http-host', HOST, '--auth-file', str(self.auth),
                                '--panel-auth-file', str(self.panel_auth)], capture_output=True, timeout=10)
            self.assertNotEqual(p.returncode, 0, content)
            self.assertNotIn(READ.encode(), p.stderr)
        self.write_panel(READ, KEY)
        self.panel_auth.chmod(0o644)
        p = subprocess.run([str(BINARY), '--data-dir', str(self.root / 'data'), 'serve', '--http-port',
                            str(self.port), '--http-host', HOST, '--auth-file', str(self.auth),
                            '--panel-auth-file', str(self.panel_auth)], capture_output=True, timeout=10)
        self.assertNotEqual(p.returncode, 0)
        self.write_panel(READ, KEY)
        self.start()


class Data(Panel):
    def setUp(self):
        super().setUp()
        self.c = self.cookie()
        base = dict(source='synthetic:panel', confirmed=True)
        self.decision = self.tool('remember', dict(base, project='alpha', operation_id='a1', agent='codex',
            kind='decision', topic='aberturas', title='Ventanas', content='Elegimos ventanas de PVC por aislamiento acústico'))
        self.tool('correct', dict(base, project='alpha', operation_id='a2', agent='claude', id=self.decision['id'],
            expected_revision=1, kind='decision', topic='aberturas', title='Ventanas',
            content='Elegimos ventanas de PVC por **aislamiento** acústico y térmico'))
        self.hostile = self.tool('remember', dict(project='alpha', operation_id='a3', agent='claude', confirmed=False,
            source='synthetic:panel', kind='note', content='<script>alert(1)</script><img src=x onerror=alert(2)> [x](javascript:alert(3))'))
        self.beta = self.tool('remember', dict(base, project='beta', operation_id='b1', agent='codex', kind='progress',
            content='Instalación de PVC en beta completada'))

    def test_search_filters_and_project_separation(self):
        status, body, headers = self.get('/panel/api/memories', self.c)
        self.assertEqual(status, 200)
        self.assertEqual(headers.get('cache-control'), 'no-store')
        self.assertEqual(headers.get('x-content-type-options'), 'nosniff')
        self.assertEqual(body['total'], 3)
        ids = {i['id']: i for i in body['items']}
        self.assertEqual(ids[self.decision['id']]['revision'], 2)
        self.assertEqual(ids[self.decision['id']]['state'], 'confirmed')
        self.assertTrue(all(i['state'] != 'replaced' for i in body['items']))
        for field in ('project', 'kind', 'agent', 'principal', 'source', 'saved_at', 'title', 'topic', 'content'):
            self.assertIn(field, ids[self.decision['id']])
        self.assertEqual(ids[self.decision['id']]['principal'], 'owner')
        def ids_for(**params):
            status, body, _ = self.get('/panel/api/memories', self.c, **params)
            self.assertEqual(status, 200, body)
            return {i['id'] for i in body['items']}
        today = datetime.datetime.now(datetime.timezone.utc).date()
        self.assertEqual(ids_for(q='PVC'), {self.decision['id'], self.beta['id']})
        self.assertEqual(ids_for(q='pv'), {self.decision['id'], self.beta['id']})
        self.assertEqual(ids_for(q='PVC', project='alpha'), {self.decision['id']})
        self.assertEqual(ids_for(q='aluminio'), set())
        self.assertEqual(ids_for(q='PVC ... -'), {self.decision['id'], self.beta['id']})
        self.assertEqual(ids_for(kind='decision'), {self.decision['id']})
        self.assertEqual(ids_for(kind='note,progress'), {self.hostile['id'], self.beta['id']})
        self.assertEqual(ids_for(agent='claude'), {self.decision['id'], self.hostile['id']})
        self.assertEqual(ids_for(principal='owner'), {self.decision['id'], self.hostile['id'], self.beta['id']})
        self.assertEqual(ids_for(principal='nadie'), set())
        self.assertEqual(ids_for(state='proposed'), {self.hostile['id']})
        self.assertEqual(ids_for(state='confirmed', project='alpha'), {self.decision['id']})
        self.assertEqual(ids_for(topic='aberturas'), {self.decision['id']})
        self.assertEqual(len(ids_for(**{'from': str(today), 'to': str(today)})), 3)
        self.assertEqual(ids_for(**{'from': str(today + datetime.timedelta(days=1))}), set())
        self.assertEqual(ids_for(to=str(today - datetime.timedelta(days=1))), set())
        for bad in ({'kind': 'secret'}, {'state': 'replaced'}, {'from': '2026-13-01'}, {'limit': '0'},
                    {'limit': '51'}, {'offset': '-1'}, {'offset': '10001'}, {'unknown': '1'}, {'q': 'x' * 513}, {'project': 'p' * 129}):
            status, body, _ = self.get('/panel/api/memories', self.c, **bad)
            self.assertEqual(status, 400, bad)
            self.assertNotIn(list(bad.values())[0], json.dumps(body))

    def test_pagination_has_no_duplicates_and_detects_changes(self):
        for n in range(25):
            self.tool('remember', dict(project='gamma', operation_id=f'g{n}', agent='test', source='synthetic',
                                       content=f'Nota sintética número {n}'))
        status, first, _ = self.get('/panel/api/memories', self.c, project='gamma', limit=20)
        self.assertEqual((status, len(first['items']), first['more'], first['total']), (200, 20, True, 25))
        status, second, _ = self.get('/panel/api/memories', self.c, project='gamma', limit=20, offset=20,
                                     snapshot=first['snapshot'])
        self.assertEqual((status, len(second['items']), second['more']), (200, 5, False))
        seen = [i['id'] for i in first['items'] + second['items']]
        self.assertEqual(len(seen), len(set(seen)))
        self.assertEqual(self.get('/panel/api/memories', self.c, offset=20)[0], 400)
        self.tool('remember', dict(project='gamma', operation_id='g-late', agent='test', source='synthetic', content='tardía'))
        status, _, _ = self.get('/panel/api/memories', self.c, project='gamma', limit=20, offset=20,
                                snapshot=first['snapshot'])
        self.assertEqual(status, 409)

    def test_detail_history_and_untrusted_content(self):
        status, body, _ = self.get('/panel/api/memory', self.c, project='alpha', id=self.decision['id'])
        self.assertEqual(status, 200)
        self.assertEqual(body['current']['revision'], 2)
        self.assertEqual([r['revision'] for r in body['revisions']], [1, 2])
        self.assertEqual([r['state'] for r in body['revisions']], ['replaced', 'confirmed'])
        self.assertEqual([r['agent'] for r in body['revisions']], ['codex', 'claude'])
        self.assertEqual([r['principal'] for r in body['revisions']], ['owner', 'owner'])
        self.assertEqual(body['created_at'], body['revisions'][0]['saved_at'])
        self.assertEqual(self.get('/panel/api/memory', self.c, project='beta', id=self.decision['id'])[0], 404)
        status, raw, headers, _ = self.http('GET', '/panel/api/memory?' + urllib.parse.urlencode(
            {'project': 'alpha', 'id': self.hostile['id']}), cookie=self.c)
        self.assertTrue(headers['content-type'].startswith('application/json'))
        self.assertEqual(json.loads(raw)['current']['content'], self.hostile['content'])

    def test_history_pages_reach_the_first_revision(self):
        memory = self.tool('remember', dict(project='hist', operation_id='h0', agent='test', source='synthetic',
                                            confirmed=True, content='revision 1'))
        for n in range(2, 62):
            self.tool('correct', dict(project='hist', operation_id=f'h{n}', agent='test', source='synthetic',
                                      confirmed=True, id=memory['id'], expected_revision=n - 1, content=f'revision {n}'))
        status, first, _ = self.get('/panel/api/memory', self.c, project='hist', id=memory['id'])
        self.assertEqual((status, len(first['revisions']), first['next_before']), (200, 50, 12))
        status, second, _ = self.get('/panel/api/memory', self.c, project='hist', id=memory['id'], before=12)
        self.assertEqual((status, second['next_before']), (200, None))
        seen = [r['revision'] for r in second['revisions'] + first['revisions']]
        self.assertEqual(seen, list(range(1, 62)))
        self.assertEqual(second['current']['revision'], 61)
        for bad in ('1', '0', 'x'):
            self.assertEqual(self.get('/panel/api/memory', self.c, project='hist', id=memory['id'], before=bad)[0], 400)

    def test_projects_facets_and_status(self):
        status, body, _ = self.get('/panel/api/projects', self.c)
        self.assertEqual(status, 200)
        projects = {p['project']: p for p in body['items']}
        self.assertEqual(set(projects), {'alpha', 'beta'})
        self.assertEqual(projects['alpha']['total'], 2)
        self.assertEqual(projects['alpha']['kinds'], {'decision': 1, 'note': 1})
        self.assertEqual(projects['alpha']['topics'], 1)
        self.assertTrue(projects['beta']['last_saved_at'])
        status, body, _ = self.get('/panel/api/facets', self.c, project='alpha')
        self.assertEqual((body['agents'], body['topics']), (['claude'], ['aberturas']))
        status, body, _ = self.get('/panel/api/facets', self.c)
        self.assertEqual(body['agents'], ['claude', 'codex'])
        self.assertEqual(body['principals'], ['owner'])
        self.assertEqual(body['projects'], ['alpha', 'beta'])
        status, body, _ = self.get('/panel/api/status', self.c)
        self.assertEqual((status, body['memories'], body['projects'], body['schema_version']), (200, 3, 2, 7))
        self.assertTrue(body['last_save'] and body['version'])


class Static(Panel):
    def test_assets_are_self_contained_and_within_budget(self):
        self.assertEqual(self.http('GET', '/panel/', origin=None)[0], 200)
        status, html, headers, _ = self.http('GET', '/panel', origin=None)
        self.assertEqual(status, 200)
        self.assertTrue(headers['content-type'].startswith('text/html'))
        csp = headers['content-security-policy']
        for directive in ("default-src 'none'", "script-src 'self'", "style-src 'self'", "connect-src 'self'", "font-src 'self'",
                          "frame-ancestors 'none'", "base-uri 'none'", "form-action 'none'"):
            self.assertIn(directive, csp)
        self.assertNotIn('unsafe', csp)
        # Images: the panel itself and Google profile photos only.
        self.assertIn("img-src 'self' https://lh3.googleusercontent.com;", csp)
        self.assertEqual(headers.get('x-content-type-options'), 'nosniff')
        self.assertEqual(headers.get('referrer-policy'), 'no-referrer')
        text = html.decode()
        self.assertNotRegex(text, r'<script(?![^>]*\ssrc=)')
        self.assertNotRegex(text, r'\sstyle=|\son[a-z]+=')
        assets = re.findall(r'(?:src|href)="([^"]+)"', text)
        self.assertTrue(assets)
        total = len(gzip.compress(html, 9))
        for asset in assets:
            self.assertFalse(re.match(r'^[a-z]+:|^//', asset), asset)
            status, body, headers, _ = self.http('GET', urllib.parse.urljoin('/panel/', asset), origin=None)
            self.assertEqual(status, 200, asset)
            self.assertEqual(headers.get('x-content-type-options'), 'nosniff')
            total += len(gzip.compress(body, 9))
            source = body.decode()
            # XML namespace identifiers are not fetched; any other URL would be a remote resource.
            self.assertNotRegex(source, r'https?://(?!www\.w3\.org/(?:2000/svg|1999/xhtml|1999/xlink|1998/Math/MathML)\b)', asset)
            if asset.endswith('.js'):
                # Assignments and calls that parse HTML or code; the framework's own
                # comparison against the string 'innerHTML' is not a sink.
                self.assertNotRegex(source, r'\.(?:innerHTML|outerHTML)\s*=|\[\s*["\'`](?:innerHTML|outerHTML)["\'`]\s*\]\s*=|insertAdjacentHTML\(|setHTMLUnsafe|createContextualFragment|DOMParser|document\.write|\beval\(|new Function|import\(|localStorage\.setItem\([^)]*(token|cookie)')
            if asset.endswith('.css'):
                # Fonts referenced by the stylesheet are served too (relative to it).
                for font in re.findall(r'url\(([^)]+)\)', source):
                    font = font.strip('\'"')
                    self.assertFalse(re.match(r'^[a-z]+:|^//', font), font)
                    status, body, headers, _ = self.http('GET', urllib.parse.urljoin(urllib.parse.urljoin('/panel/', asset), font), origin=None)
                    self.assertEqual((status, headers.get('content-type'), body[:4]), (200, 'font/woff2', b'wOF2'), font)
        self.assertLessEqual(total, 200 * 1024)
        # Panel sources never render raw HTML from data.
        sources = Path(__file__).resolve().parent.parent / 'panel-ui' / 'src'
        for path in sources.rglob('*.svelte'):
            self.assertNotIn('{@html', path.read_text(), path)
        self.assertEqual(self.http('GET', '/panel/../mcp', origin=None)[0] in (400, 404), True)
        self.assertEqual(self.http('GET', '/panel/missing.js', origin=None)[0], 404)


class Disabled(Panel):
    panel = False

    def test_panel_absent_without_credentials(self):
        for path in ('/panel', '/panel/api/session'):
            self.assertEqual(self.http('GET', path)[0], 404)
        self.assertEqual(self.login()[0], 404)
        self.assertEqual(self.mcp_status(), 200)


if __name__ == '__main__':
    unittest.main()
