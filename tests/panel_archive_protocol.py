"""Panel archive, restore and purge: archived memories leave every MCP read and panel
listing until restored; only the owner purges, and nothing of a purged memory stays in
the live database. Reuses the simulated-Google harness; all data is synthetic."""
import json
import sqlite3
import unittest
import uuid

import panel_google_protocol as base

ANA = base.ANA
ORIGIN = base.ORIGIN
MARK = 'zorzal-violeta-7731'


class Archive(base.PanelGoogle):
    def csrf(self, cookie):
        return self.get(cookie, 'session')[1].get('csrf')

    def post(self, cookie, view, body, csrf=True, origin=ORIGIN):
        headers = {'Cookie': cookie, 'Content-Type': 'application/json'}
        if origin:
            headers['Origin'] = origin
        if csrf is True:
            csrf = self.csrf(cookie)
        if csrf:
            headers['X-CSRF-Token'] = csrf
        status, raw, _ = self.http('POST', f'/panel/api/{view}', json.dumps(body), headers)
        return status, json.loads(raw) if raw else None

    def marked(self, topic='zorzal'):
        """Ana's memory with a unique marker in title, content and topic."""
        saved = self.tool(ANA, 'remember', {
            'project': 'notas', 'content': f'Contenido {MARK}', 'title': f'Título {MARK}', 'kind': 'decision',
            'topic': topic, 'source': 'test', 'agent': 'codex', 'operation_id': str(uuid.uuid4())})
        corrected = self.tool(ANA, 'correct', {
            'project': 'notas', 'id': saved['id'], 'expected_revision': 1, 'content': f'Segunda {MARK}',
            'source': 'test', 'agent': 'codex', 'operation_id': str(uuid.uuid4())})
        return corrected

    def mcp_error(self, name, args):
        rpc = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': name, 'arguments': args}}
        status, body, _ = self.http('POST', '/mcp', json.dumps(rpc), {
            'Accept': 'application/json, text/event-stream', 'Content-Type': 'application/json',
            'MCP-Protocol-Version': '2025-03-26', 'Authorization': 'Bearer ' + ANA})
        self.assertEqual(status, 200)
        payload = json.loads(body)
        if 'error' in payload:
            return payload['error']['message']
        result = payload['result']
        self.assertTrue(result.get('isError'), result)
        return result['content'][0]['text']

    def mcp_sees(self, memory):
        search = self.tool(ANA, 'search', {'project': 'notas', 'query': MARK})
        context = self.tool(ANA, 'context', {'project': 'notas', 'limit': 20})
        found = [m['id'] for m in search['items']], [m['id'] for m in context['items']]
        return memory['id'] in found[0] and memory['id'] in found[1]

    def listed(self, cookie, **params):
        return [m['id'] for m in self.get(cookie, 'memories', **params)[1]['items']]

    def test_archive_hides_everywhere_until_restored(self):
        memory = self.marked()
        ana = self.google_session('ana@example.test')
        before = self.get(ana, 'memories')[1]
        status_before = self.get(ana, 'status')[1]
        self.assertTrue(self.mcp_sees(memory))
        status, body = self.post(ana, 'archive', {'project': 'notas', 'id': memory['id'], 'archived': True})
        self.assertEqual((status, body['archived']), (200, True), body)
        # MCP: search, context, recall (any revision), history and correct miss it.
        self.assertFalse(self.mcp_sees(memory))
        self.assertIn('not found', self.mcp_error('recall', {'project': 'notas', 'id': memory['id']}))
        self.assertIn('not found', self.mcp_error('recall', {'project': 'notas', 'id': memory['id'],
                                                             'revision': 1, 'compact': True}))
        self.assertIn('not found', self.mcp_error('history', {'project': 'notas', 'id': memory['id']}))
        self.assertIn('revision conflict', self.mcp_error('correct', {
            'project': 'notas', 'id': memory['id'], 'expected_revision': 2, 'content': 'x', 'source': 't',
            'agent': 't', 'operation_id': str(uuid.uuid4())}))
        self.assertIn('archived', self.mcp_error('remember', {
            'project': 'notas', 'topic': 'zorzal', 'content': 'otra', 'source': 't', 'agent': 't',
            'operation_id': str(uuid.uuid4())}))
        self.assertEqual(self.tool(ANA, 'status', {})['memories'], 2)
        # Panel: gone from listings and counts, present under archived, detail still opens.
        self.assertNotIn(memory['id'], self.listed(ana))
        self.assertNotIn(memory['id'], self.listed(ana, q=MARK))
        self.assertEqual(self.listed(ana, archived='1'), [memory['id']])
        status_now = self.get(ana, 'status')[1]
        self.assertEqual((status_now['memories'], status_now['archived']),
                         (status_before['memories'] - 1, 1))
        self.assertEqual(self.get(ana, 'memories', snapshot=str(before['snapshot']), offset='1')[0], 409)
        detail = self.get(ana, 'memory', project='notas', id=memory['id'])[1]
        self.assertTrue(detail['archived_at'])
        self.assertTrue(detail['can_purge'])
        # Restoring brings everything back.
        status, body = self.post(ana, 'archive', {'project': 'notas', 'id': memory['id'], 'archived': False})
        self.assertEqual((status, body['archived']), (200, False))
        self.assertTrue(self.mcp_sees(memory))
        self.assertIn(memory['id'], self.listed(ana))
        self.assertEqual(self.listed(ana, archived='1'), [])
        detail = self.get(ana, 'memory', project='notas', id=memory['id'])[1]
        self.assertIsNone(detail['archived_at'])
        self.assertFalse(detail['can_purge'])

    def test_who_may_archive_and_purge(self):
        memory = self.marked()
        body = {'project': 'ana/notas', 'id': memory['id'], 'archived': True}
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'read'})
        bob = self.google_session('bob@example.test')
        self.assertEqual(self.post(bob, 'archive', body)[0], 403)
        self.assertEqual(self.post(bob, 'archive', {**body, 'project': 'ana/diario'})[0], 403)
        # Without the token or from another origin nothing changes.
        ana = self.google_session('ana@example.test')
        own = {**body, 'project': 'notas'}
        self.assertEqual(self.post(ana, 'archive', own, csrf=None)[0], 403)
        self.assertEqual(self.post(ana, 'archive', own, origin='https://evil.example.test')[0], 403)
        self.assertEqual(self.post(self.read_key_session(), 'archive', own, csrf='f' * 64)[0], 403)
        self.assertEqual(self.listed(ana, archived='1'), [])
        # A write member archives and restores, but cannot purge.
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'write'})
        self.assertEqual(self.post(bob, 'archive', body)[0], 200)
        self.assertFalse(self.get(bob, 'memory', project='ana/notas', id=memory['id'])[1]['can_purge'])
        purge = {'project': 'ana/notas', 'id': memory['id'], 'confirm': 'purgar'}
        self.assertEqual(self.post(bob, 'purge', purge)[0], 403)
        # Agents have no archive or purge tool.
        for name in ('archive', 'purge', 'panel.archive', 'panel.purge'):
            self.assertRegex(self.mcp_error(name, {'project': 'notas', 'id': memory['id']}).lower(),
                             'unknown tool|tool unavailable')
        self.assertEqual(self.post(bob, 'archive', {**body, 'archived': False})[0], 200)

    def test_owner_purges_an_archived_memory_without_a_trace(self):
        memory = self.marked()
        ana = self.google_session('ana@example.test')
        purge = {'project': 'notas', 'id': memory['id'], 'confirm': 'purgar'}
        # Only archived memories, and only with the confirmation word.
        self.assertEqual(self.post(ana, 'purge', purge)[0], 409)
        self.assertEqual(self.post(ana, 'archive', {**purge, 'archived': True, 'confirm': None})[0], 400)
        self.assertEqual(self.post(ana, 'archive', {'project': 'notas', 'id': memory['id'], 'archived': True})[0], 200)
        self.assertEqual(self.post(ana, 'purge', {**purge, 'confirm': 'borrar'})[0], 400)
        self.assertEqual(self.post(ana, 'purge', purge, csrf=None)[0], 403)
        status, body = self.post(ana, 'purge', purge)
        self.assertEqual((status, body['purged']), (200, True), body)
        self.assertEqual(self.get(ana, 'memory', project='notas', id=memory['id'])[0], 404)
        self.assertEqual(self.listed(ana, archived='1'), [])
        self.assertIn('not found', self.mcp_error('recall', {'project': 'notas', 'id': memory['id']}))
        self.assertEqual(self.post(ana, 'purge', purge)[0], 404)
        # The topic is free again.
        self.tool(ANA, 'remember', {'project': 'notas', 'topic': 'zorzal', 'content': 'nuevo', 'source': 't',
                                    'agent': 't', 'operation_id': str(uuid.uuid4())})
        # Nothing of the content remains in the database or its WAL; only a bare record.
        for suffix in ('', '-wal'):
            path = self.root / 'data' / ('memory.sqlite' + suffix)
            if path.exists():
                self.assertNotIn(MARK.encode(), path.read_bytes(), suffix)
        db = sqlite3.connect(f'file:{self.root / "data" / "memory.sqlite"}?mode=ro', uri=True)
        try:
            rows = db.execute('SELECT * FROM purges').fetchall()
            columns = [c[1] for c in db.execute('PRAGMA table_info(purges)')]
            leftovers = db.execute(
                "SELECT (SELECT count(*) FROM revisions WHERE id=?1)+(SELECT count(*) FROM revision_metadata WHERE id=?1)"
                "+(SELECT count(*) FROM topics WHERE id=?1)+(SELECT count(*) FROM memory_fts WHERE id=?1)"
                "+(SELECT count(*) FROM operations WHERE response LIKE '%'||?1||'%' OR request LIKE '%'||?1||'%')",
                [memory['id']]).fetchone()[0]
        finally:
            db.close()
        self.assertEqual(columns, ['id', 'project', 'purged_by', 'purged_at'])
        self.assertEqual([r[:3] for r in rows], [(memory['id'], 'ana/notas', 'ana')])
        self.assertEqual(leftovers, 0)

    def test_snapshots_never_repeat_and_facets_and_retries_hide_archived(self):
        op = str(uuid.uuid4())
        args = {'project': 'notas', 'content': f'Contenido {MARK}', 'topic': 'tema-oculto', 'source': 'test',
                'agent': 'agente-oculto', 'operation_id': op}
        saved = self.tool(ANA, 'remember', args)
        self.tool(ANA, 'correct', {'project': 'notas', 'id': saved['id'], 'expected_revision': 1, 'content': 'dos',
                                   'source': 'test', 'agent': 'agente-oculto', 'operation_id': str(uuid.uuid4())})
        ana = self.google_session('ana@example.test')
        snapshots = [self.tool(ANA, 'context', {'project': 'notas', 'limit': 1})['snapshot']]
        body = {'project': 'notas', 'id': saved['id']}
        self.assertEqual(self.post(ana, 'archive', {**body, 'archived': True})[0], 200)
        snapshots.append(self.tool(ANA, 'context', {'project': 'notas', 'limit': 1})['snapshot'])
        # Facets (filters) no longer offer the archived memory's agent or topic.
        facets = self.get(ana, 'facets')[1]
        self.assertNotIn('agente-oculto', facets['agents'])
        self.assertNotIn('tema-oculto', facets['topics'])
        archived = self.get(ana, 'facets', archived='1')[1]
        self.assertEqual((archived['agents'], archived['topics']), (['agente-oculto'], ['tema-oculto']))
        # Retrying the original remember does not bring its content back.
        self.assertIn('not found', self.mcp_error('remember', args))
        self.assertEqual(self.post(ana, 'purge', {**body, 'confirm': 'purgar'})[0], 200)
        # After a purge, retrying the original remember is refused, not recreated.
        self.assertIn('operation_id already used', self.mcp_error('remember', args))
        self.assertEqual(self.tool(ANA, 'search', {'project': 'notas', 'query': MARK})['items'], [])
        snapshots.append(self.tool(ANA, 'context', {'project': 'notas', 'limit': 1})['snapshot'])
        for n in range(3):
            self.remember(ANA, 'notas', f'nueva {n}')
            snapshots.append(self.tool(ANA, 'context', {'project': 'notas', 'limit': 1})['snapshot'])
        self.assertEqual(snapshots, sorted(set(snapshots)), snapshots)
        # A continuation with the pre-archive snapshot is refused.
        self.assertIn('restart listing', self.mcp_error('context', {'project': 'notas', 'limit': 1, 'offset': 1,
                                                                    'snapshot': snapshots[0]}))


# Run only the archive tests here; the inherited ones run in panel_google_protocol.py.
for _name in dir(base.PanelGoogle):
    if _name.startswith('test_'):
        setattr(Archive, _name, None)


if __name__ == '__main__':
    unittest.main()
