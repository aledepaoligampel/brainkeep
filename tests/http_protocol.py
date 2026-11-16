"""Public HTTP boundary tests; all credentials and memories are synthetic."""
import concurrent.futures
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
TOKEN = 'a' * 64

class HttpTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='brain-http-')
        self.root = Path(self.temp.name)
        self.auth = self.root/'auth.json'
        self.auth.write_text(json.dumps({'test': TOKEN, 'second': 'b'*64}))
        self.auth.chmod(0o600)
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            self.port = s.getsockname()[1]
        self.logs = open(self.root/'log', 'wb')
        self.start()

    def start(self):
        self.daemon = subprocess.Popen([str(BINARY), '--data-dir', str(self.root/'data'),
            'serve', '--http-port', str(self.port), '--http-host', 'memory.example.test',
            '--auth-file', str(self.auth)], stdout=subprocess.DEVNULL, stderr=self.logs)
        for _ in range(100):
            if self.daemon.poll() is not None:
                raise AssertionError('HTTP daemon failed to start: '+(self.root/'log').read_text())
            try:
                if self.request(token=None)[0] == 401: return
            except OSError: pass
            time.sleep(.05)
        raise AssertionError('HTTP startup timeout')

    def tearDown(self):
        if hasattr(self, 'daemon') and self.daemon.poll() is None:
            self.daemon.terminate()
            self.daemon.wait(timeout=10)
        self.logs.close()
        self.temp.cleanup()

    def request(self, rpc=None, token=TOKEN, host='memory.example.test', origin=None, raw=None):
        c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        headers = {'Host':host, 'Accept':'application/json, text/event-stream',
                   'Content-Type':'application/json', 'MCP-Protocol-Version':'2025-03-26'}
        if token is not None: headers['Authorization'] = 'Bearer '+token
        if origin is not None: headers['Origin'] = origin
        body = raw if raw is not None else json.dumps(rpc or {'jsonrpc':'2.0','id':1,'method':'tools/list'})
        c.request('POST', '/mcp', body, headers)
        r=c.getresponse(); out=(r.status,r.read(),{k.lower():v for k,v in r.getheaders()}); c.close()
        return out

    def tool(self, name, args, token=TOKEN):
        status, body, _ = self.request({'jsonrpc':'2.0','id':2,'method':'tools/call',
            'params':{'name':name,'arguments':args}}, token=token)
        self.assertEqual(status,200,body)
        result=json.loads(body)['result']
        if result.get('isError'): raise ValueError(result['content'][0]['text'])
        return json.loads(result['content'][0]['text'])['data']

    def users(self, *args):
        out = subprocess.run([str(BINARY), '--data-dir', str(self.root/'data'), 'users', *args],
            capture_output=True, text=True, timeout=10)
        self.assertEqual(out.returncode, 0, out.stderr)
        return json.loads(out.stdout)

    def test_user_tokens_follow_invitation_and_disable_without_restart(self):
        ana = 'c'*64
        self.daemon.terminate(); self.daemon.wait(timeout=10)
        self.auth.write_text(json.dumps({'test': TOKEN, 'ana-laptop': {'token': ana, 'user': 'ana', 'read': ['*'], 'write': ['*']}}))
        self.start()
        self.assertEqual(self.request(token=ana)[0], 401)
        self.users('invite', 'ana@example.test', '--handle', 'ana')
        self.assertEqual(self.request(token=ana)[0], 200)
        saved = self.tool('remember', {'project':'demo','operation_id':'a1','content':'Nota sintética de ana','source':'synthetic:http','agent':'t'}, token=ana)
        self.assertEqual((saved['project'], saved['principal']), ('demo', 'ana'))
        self.assertEqual(self.tool('status', {})['memories'], 0)
        self.assertEqual([u['handle'] for u in self.users('list')['users']], ['owner', 'ana'])
        self.users('disable', 'ana')
        status, _, headers = self.request(token=ana)
        self.assertEqual((status, headers.get('www-authenticate')), (401, 'Bearer realm="brainkeep"'))
        self.assertEqual(self.request()[0], 200)
        # Remote credentials cannot reach user administration.
        with self.assertRaises(ValueError):
            self.tool('users.list', {})

    def test_auth_origin_host_body_and_admin(self):
        for token in (None, '', 'c'*64, TOKEN+'x'):
            status, body, headers=self.request(token=token)
            self.assertEqual(status,401)
            self.assertNotIn(TOKEN.encode(),body)
            self.assertEqual(headers.get('cache-control'),'no-store')
        self.assertEqual(self.request(host='evil.example')[0],403)
        self.assertEqual(self.request(origin='https://evil.example')[0],403)
        self.assertEqual(self.request(origin='null')[0],403)
        self.assertEqual(self.request(raw=' '*65537)[0],413)
        status, body, _ = self.request()
        self.assertEqual(status,200,body)
        self.assertEqual(len(json.loads(body)['result']['tools']),11)
        with self.assertRaises(ValueError):
            self.tool('backup',{'destination':str(self.root/'forbidden.sqlite')})
        self.assertFalse((self.root/'forbidden.sqlite').exists())

    def test_cross_credential_retry_restart_and_revocation(self):
        note=dict(project='remote-test',operation_id='remote-one',content='Synthetic remote SQLite decision',
                  source='synthetic:http',agent='test',confirmed=True)
        with concurrent.futures.ThreadPoolExecutor(4) as pool:
            rows=list(pool.map(lambda _: self.tool('remember',note),range(4)))
        self.assertTrue(all(row==rows[0] for row in rows))
        first=rows[0]
        self.assertEqual(self.tool('recall',{'project':'remote-test','id':first['id']},'b'*64), first)
        self.daemon.terminate(); self.daemon.wait(timeout=10)
        self.auth.write_text(json.dumps({'second':'b'*64}))
        self.start()
        self.assertEqual(self.request()[0],401)
        self.assertEqual(self.tool('recall',{'project':'remote-test','id':first['id']},'b'*64),first)

    def test_duplicate_security_headers_rejected(self):
        for duplicate in ('Host','Origin','Authorization'):
            c=http.client.HTTPConnection('127.0.0.1',self.port,timeout=5)
            body=json.dumps({'jsonrpc':'2.0','id':1,'method':'tools/list'})
            c.putrequest('POST','/mcp',skip_host=True)
            c.putheader('Host','memory.example.test')
            c.putheader('Origin','https://memory.example.test')
            c.putheader('Authorization','Bearer '+TOKEN)
            c.putheader(duplicate, {'Host':'evil.test','Origin':'https://evil.test',
                'Authorization':'Bearer '+'c'*64}[duplicate])
            c.putheader('Content-Type','application/json')
            c.putheader('Accept','application/json, text/event-stream')
            c.putheader('Content-Length',str(len(body)))
            c.endheaders(body.encode())
            response=c.getresponse(); response.read()
            c.close()
            self.assertEqual(response.status,401 if duplicate=='Authorization' else 403,duplicate)

    def test_team_principal_and_project_permissions(self):
        self.daemon.terminate(); self.daemon.wait(timeout=10)
        ana, bob = 'd'*64, 'e'*64
        self.auth.write_text(json.dumps({'test':TOKEN,
            'ana':{'token':ana,'read':['equipo/*'],'write':['equipo/api']},
            'bob':{'token':bob,'read':['equipo/web']}}))
        self.start()
        note=lambda project,op: dict(project=project,operation_id=op,content='Synthetic team decision',
            source='synthetic:http',agent='declared-by-agent')
        saved=self.tool('remember',note('equipo/api','team-1'),ana)
        self.assertEqual((saved['principal'],saved['agent']),('owner','declared-by-agent'))
        self.tool('remember',note('equipo/web','team-2'),TOKEN)
        self.tool('remember',note('personal','team-3'),TOKEN)
        for token,project in ((ana,'equipo/web'),(bob,'equipo/web'),(bob,'personal')):
            with self.assertRaisesRegex(ValueError,'not accessible'):
                self.tool('remember',note(project,'denied-'+token[0]+project),token)
        for project in ('equipo/api','personal','missing'):
            with self.assertRaisesRegex(ValueError,'not accessible'):
                self.tool('context',{'project':project},bob)
        self.assertEqual(len(self.tool('context',{'project':'equipo/web'},bob)['items']),1)
        self.assertEqual([self.tool('status',{},t)['memories'] for t in (TOKEN,ana,bob)],[3,2,1])
        history=self.tool('history',{'project':'equipo/api','id':saved['id']},TOKEN)
        self.assertEqual(history['items'][0]['principal'],'owner')
        guide=self.tool('guide',{},ana)
        self.assertEqual((guide['principal'],guide['read'],guide['write']),('owner',['equipo/*'],['equipo/api']))
        self.assertIn('CLAUDE.md',guide['guide'])
        self.assertNotIn('personal',json.dumps(guide))
        self.assertLessEqual(len(json.dumps(guide,ensure_ascii=False).encode()),4096)
        self.assertEqual(self.tool('guide',{'compact':True},bob)['read'],['equipo/web'])

    def test_invalid_team_configuration_refuses_start(self):
        self.daemon.terminate(); self.daemon.wait(timeout=10)
        self.auth.write_text(json.dumps({'a':TOKEN,'b':{'token':TOKEN,'read':['*']}}))
        with self.assertRaisesRegex(AssertionError,'failed to start'):
            self.start()

    def test_slow_bodies_of_one_user_do_not_starve_others(self):
        """One user holding many incomplete request bodies, through any number of its
        credentials, only blocks itself; incomplete bodies are cut after a few seconds
        (evaluation finding H1)."""
        laptop, desktop = 'c'*64, 'd'*64
        self.daemon.terminate(); self.daemon.wait(timeout=10)
        self.auth.write_text(json.dumps({'test': TOKEN, 'second': 'b'*64,
            'ana-laptop': {'token': laptop, 'user': 'ana', 'read': ['*'], 'write': ['*']},
            'ana-desktop': {'token': desktop, 'user': 'ana', 'read': ['*'], 'write': ['*']}}))
        self.start()
        self.users('invite', 'ana@example.test', '--handle', 'ana')
        held = []
        try:
            # Ana spreads 20 incomplete bodies over two credentials.
            for n in range(20):
                s = socket.create_connection(('127.0.0.1', self.port), timeout=10)
                s.sendall(('POST /mcp HTTP/1.1\r\nHost: memory.example.test\r\nAuthorization: Bearer ' +
                           (laptop if n % 2 else desktop) +
                           '\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{').encode())
                held.append(s)
            time.sleep(0.3)
            # Other credentials are unaffected; Ana is over her share on any credential.
            self.assertEqual([self.request(token=t)[0] for t in (TOKEN, 'b'*64, TOKEN)], [200, 200, 200])
            status, _, headers = self.request(token=laptop)
            self.assertEqual((status, headers.get('retry-after')), (429, '1'))
            # Held bodies are answered (cut) within the body timeout, not 15 s.
            start = time.monotonic()
            statuses = []
            for s in held:
                s.settimeout(10)
                statuses.append(s.recv(64).split(b' ')[1])
            self.assertLess(time.monotonic() - start, 8)
            self.assertEqual(sorted(set(statuses)), [b'408', b'429'])
            self.assertEqual(statuses.count(b'408'), 6)
            self.assertEqual(self.request(token=laptop)[0], 200)
        finally:
            for s in held:
                s.close()

    def test_read_only_credentials_supported(self):
        self.daemon.terminate();self.daemon.wait(timeout=10)
        self.auth.chmod(0o400)
        self.start()
        self.assertEqual(self.request()[0],200)

if __name__ == '__main__': unittest.main()
