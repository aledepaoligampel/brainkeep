"""Black-box MCP/CLI checks. These clients are test drivers, NOT real agent validation."""
import concurrent.futures
import json
import os
from pathlib import Path
import select
import stat
import subprocess
import tempfile
import time
import unittest

BINARY = Path(os.environ.get('BRAINKEEP_BINARY', 'target/debug/brainkeep')).resolve()

class Client:
    def __init__(self, directory):
        self.p = subprocess.Popen([str(BINARY), '--data-dir', str(directory), 'mcp'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
        self.seq = 0
        self.info = self.request('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {}, 'clientInfo': {'name': 'brainkeep-test-driver', 'version': '1'}})
        self.p.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n')
        self.p.stdin.flush()
    def request(self, method, params):
        self.seq += 1
        self.p.stdin.write(json.dumps({'jsonrpc':'2.0','id':self.seq,'method':method,'params':params})+'\n')
        self.p.stdin.flush()
        while True:
            if not select.select([self.p.stdout], [], [], 10)[0]:
                raise AssertionError('MCP response timeout')
            line = self.p.stdout.readline()
            if not line: raise AssertionError('MCP transport closed')
            response = json.loads(line)
            if response.get('id') == self.seq:
                assert 'error' not in response, response
                return response['result']
    def call(self, name, args):
        result = self.request('tools/call', {'name':name, 'arguments':args})
        if result.get('isError'): raise ValueError(result['content'][0]['text'])
        return json.loads(result['content'][0]['text'])['data']
    def close(self):
        if self.p.poll() is None:
            self.p.terminate()
            try: self.p.wait(timeout=3)
            except subprocess.TimeoutExpired: self.p.kill(); self.p.wait()
        for f in (self.p.stdin, self.p.stdout, self.p.stderr): f.close()

class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='brain-protocol-')
        self.root = Path(self.temp.name)
        self.data = self.root/'live'
        self.clients = []
        self.logs = open(self.root/'daemon.log', 'wb')
        self.start()
    def start(self):
        self.daemon = subprocess.Popen([str(BINARY),'--data-dir',str(self.data),'serve'], stdout=subprocess.DEVNULL, stderr=self.logs)
        end = time.monotonic()+10
        while time.monotonic()<end:
            if self.daemon.poll() is not None: raise AssertionError('daemon startup failed')
            if (self.data/'brainkeep.sock').exists():
                result = subprocess.run([str(BINARY),'--data-dir',str(self.data),'status'],capture_output=True,timeout=3)
                if result.returncode==0: return
            time.sleep(.05)
        raise AssertionError('daemon did not become ready')
    def client(self):
        c=Client(self.data); self.clients.append(c); return c
    def tearDown(self):
        for c in self.clients: c.close()
        if hasattr(self,'daemon') and self.daemon.poll() is None:
            self.daemon.terminate()
            try: self.daemon.wait(timeout=5)
            except subprocess.TimeoutExpired: self.daemon.kill(); self.daemon.wait()
        self.logs.close(); self.temp.cleanup()
    def note(self, op='save', content='Usar SQLite con FTS5'):
        return dict(project='demo',operation_id=op,content=content,source='synthetic:protocol',agent='test')
    def test_three_clients_correction_race_retry_and_recovery(self):
        clients=[self.client() for _ in range(3)]
        first=clients[0].call('remember',self.note())
        self.assertEqual(first,clients[1].call('recall',dict(project='demo',id=first['id'])))
        def correction(i):
            args=self.note(f'correct-{i}',f'Revisión concurrente {i}')
            args.update(id=first['id'],expected_revision=1,confirmed=True)
            try: return clients[i].call('correct',args)
            except ValueError: return None
        with concurrent.futures.ThreadPoolExecutor(3) as pool:
            results=list(pool.map(correction,range(3)))
        self.assertEqual(sum(r is not None for r in results),1)
        current=next(r for r in results if r is not None)
        with concurrent.futures.ThreadPoolExecutor(3) as pool:
            saved=list(pool.map(lambda i:clients[i].call('remember',self.note(f'parallel-{i}')),range(3)))
        self.assertEqual(len({x['id'] for x in saved}),3)
        self.assertEqual(clients[0].call('remember',self.note()),first)
        self.daemon.kill(); self.daemon.wait()
        for c in self.clients: c.close()
        self.clients=[]; self.start()
        c=self.client()
        self.assertEqual(c.call('recall',dict(project='demo',id=first['id'])),current)
        self.assertEqual(c.call('status',{})['memories'],4)
        self.assertEqual(c.call('history',dict(project='demo',id=first['id']))['items'][0]['state'],'replaced')
    def test_server_identity_for_chat_clients(self):
        info = self.client().info['serverInfo']
        self.assertEqual((info['name'], info.get('title')), ('brainkeep', '🧠 Brainkeep'))
    def test_full_history_handles_valid_escaped_content(self):
        c=self.client()
        content=chr(92)*16000
        first=c.call('remember',self.note('large-history-1',content))
        for revision in range(2,21):
            args=self.note(f'large-history-{revision}',content)
            args.update(id=first['id'],expected_revision=revision-1)
            c.call('correct',args)
        result=subprocess.run([str(BINARY),'--data-dir',str(self.data),'call','history'],input=json.dumps(dict(project='demo',id=first['id'],limit=20)),text=True,capture_output=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)
        items=json.loads(result.stdout)['items']
        self.assertEqual(len(items),20)
        self.assertTrue(all(item['content']==content for item in items))

    def test_secrets_permissions_and_backup(self):
        c=self.client()
        fixture='password=demo-fictional-9438127'
        for field in ['content','source','agent','project','operation_id']:
            args=self.note(); args[field]=fixture
            with self.assertRaises(ValueError) as err: c.call('remember',args)
            self.assertNotIn(fixture,str(err.exception))
        c.call('remember',self.note('ssh','Conectar con ssh server1 usando ssh-agent'))
        backup=self.root/'backup.sqlite'
        subprocess.run([str(BINARY),'--data-dir',str(self.data),'backup',str(backup)],check=True,capture_output=True,timeout=10)
        for path in list(self.data.iterdir())+[backup,self.root/'daemon.log']:
            if path.is_file():
                self.assertNotIn(fixture.encode(),path.read_bytes())
                if path.name!='daemon.log': self.assertEqual(stat.S_IMODE(path.stat().st_mode),0o600)
        self.assertEqual(stat.S_IMODE(self.data.stat().st_mode),0o700)
        self.assertEqual(stat.S_IMODE((self.data/'brainkeep.sock').stat().st_mode),0o600)
        restored=self.root/'restored'
        subprocess.run([str(BINARY),'--data-dir',str(restored),'restore',str(backup)],check=True,capture_output=True,timeout=10)
        old=(restored/'memory.sqlite').read_bytes()
        failed=subprocess.run([str(BINARY),'--data-dir',str(restored),'restore',str(backup)],capture_output=True,timeout=10)
        self.assertNotEqual(failed.returncode,0)
        self.assertEqual((restored/'memory.sqlite').read_bytes(),old)
    def test_search_limits_schema_and_invalid_operations(self):
        c=self.client()
        listed=c.request('tools/list',{})['tools']
        names={t['name'] for t in listed}
        self.assertEqual(names,{'remember','correct','recall','history','search','context','status','guide','projects','share','unshare'})
        # Memory tools load upfront in clients that defer MCP tools (Claude Code tool search).
        eager={t['name'] for t in listed if (t.get('_meta') or {}).get('anthropic/alwaysLoad') is True}
        self.assertEqual(eager,{'context','recall','remember','correct'})
        for i in range(4): c.call('remember',self.note(f'limit-{i}','SQLite FTS5 '+('a'*900)))
        result=c.call('context',dict(project='demo',limit=2))
        self.assertEqual(len(result['items']),2)
        self.assertTrue(result['more_available'])
        self.assertTrue(all(x['truncated'] for x in result['items']))
        self.assertEqual(len(c.call('search',dict(project='demo',query='SQLite FTS5'))['items']),4)
        self.assertEqual(c.call('search',dict(project='other',query='SQLite'))['items'],[])
        for args in [dict(project='demo',limit=0),dict(project='demo',limit=21),dict(project='demo',limit='five')]:
            with self.assertRaises(ValueError): c.call('context',args)
        second=subprocess.run([str(BINARY),'--data-dir',str(self.data),'serve'],capture_output=True,timeout=3)
        self.assertNotEqual(second.returncode,0)

if __name__=='__main__': unittest.main()
