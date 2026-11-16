"""Compact contract through real MCP, using disposable services only."""
import concurrent.futures
import json
import unittest
import protocol

class CompactTests(unittest.TestCase):
    setUp = protocol.ProtocolTests.setUp
    start = protocol.ProtocolTests.start
    client = protocol.ProtocolTests.client
    tearDown = protocol.ProtocolTests.tearDown
    note = protocol.ProtocolTests.note

    def test_wire_budget_paging_and_revision_pinning(self):
        c=self.client()
        init=c.request('initialize', {'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'compact-test','version':'1'}})
        self.assertLessEqual(len(init['instructions'].encode()),1536)
        self.assertIn('links to shared artifacts/docs', init['instructions'])
        catalog=c.request('tools/list',{})
        self.assertLessEqual(len(json.dumps(catalog,ensure_ascii=False,separators=(',',':')).encode()),8192)
        content=('🦀"\\\tá'*1200)
        n=self.note('unicode',content);n.update(compact=True,title='Unicode',kind='decision',topic='test/unicode',source='"'*1000)
        raw=c.request('tools/call',dict(name='remember',arguments=n))
        self.assertFalse(raw.get('isError'));self.assertLessEqual(len(raw['content'][0]['text'].encode()),512)
        saved=json.loads(raw['content'][0]['text'])['data']
        page=c.call('recall',dict(project='demo',id=saved['id'],compact=True));text=page['content']
        correction=self.note('correction','Nueva decisión');correction.update(id=saved['id'],expected_revision=1)
        c.call('correct',correction)
        while page['next_offset'] is not None:
            raw=c.request('tools/call',dict(name='recall',arguments=dict(project='demo',id=saved['id'],compact=True,revision=1,offset=page['next_offset'])))
            self.assertLessEqual(len(raw['content'][0]['text'].encode()),16384)
            page=json.loads(raw['content'][0]['text'])['data'];text+=page['content']
        self.assertEqual(text,content)
        with self.assertRaises(ValueError):c.call('recall',dict(project='demo',id=saved['id'],compact=True,offset=1))
        for i in range(12):
            n=self.note(f'page-{i}','SQLite '+('á"\\'*2500));n.update(source='"'*1000,title='Title',kind='progress',topic=f'progress/{i}')
            c.call('remember',n)
        seen=[];offset=0;snapshot=None
        while True:
            raw=c.request('tools/call',dict(name='search',arguments=dict(project='demo',query='SQLite',compact=True,limit=20,offset=offset,**({} if snapshot is None else dict(snapshot=snapshot)))))
            self.assertLessEqual(len(raw['content'][0]['text'].encode()),2048)
            out=json.loads(raw['content'][0]['text'])['data'];seen.extend(x['id'] for x in out['items'])
            if out['next_offset'] is None:break
            self.assertGreater(out['next_offset'],offset);offset=out['next_offset'];snapshot=out['snapshot']
        self.assertEqual(len(seen),12);self.assertEqual(len(set(seen)),12)

    def test_topic_concurrency_no_duplicates_or_silent_replacement(self):
        clients=[self.client() for _ in range(3)]
        def save(i):
            n=self.note(f'concurrent-{i}','Decisión de persistencia');n.update(topic='storage',kind='decision',compact=True)
            return clients[i].call('remember',n)
        with concurrent.futures.ThreadPoolExecutor(3) as pool:results=list(pool.map(save,range(3)))
        self.assertEqual(len({x['id'] for x in results}),1)
        self.assertEqual(clients[0].call('status',{})['memories'],1)
        def correct(i):
            n=self.note(f'correct-{i}',f'Alternativa {i}');n.update(id=results[0]['id'],expected_revision=1,compact=True)
            try:return clients[i].call('correct',n)
            except ValueError:return None
        with concurrent.futures.ThreadPoolExecutor(3) as pool:results=list(pool.map(correct,range(3)))
        self.assertEqual(sum(x is not None for x in results),1)

if __name__=='__main__':unittest.main()
