"""Panel edits (correct): owner or write member only, revision conflicts, Origin and
anti-CSRF token required, read key stays read-only. Reuses the simulated-Google
harness; all identities, credentials and memories are synthetic."""
import json
import uuid
import unittest

import panel_google_protocol as base

ANA = base.ANA
ORIGIN = base.ORIGIN


class Edit(base.PanelGoogle):
    def csrf(self, cookie):
        status, session = self.get(cookie, 'session')
        self.assertEqual(status, 200)
        return session.get('csrf')

    def post(self, cookie, body, csrf=None, origin=ORIGIN, content_type='application/json'):
        headers = {'Cookie': cookie, 'Content-Type': content_type}
        if origin is not None:
            headers['Origin'] = origin
        if csrf is not None:
            headers['X-CSRF-Token'] = csrf
        status, raw, _ = self.http('POST', '/panel/api/correct', json.dumps(body), headers)
        return status, json.loads(raw) if raw else None

    def first(self, cookie, project):
        [item] = self.get(cookie, 'memories', project=project)[1]['items']
        detail = self.get(cookie, 'memory', project=project, id=item['id'])[1]
        return {**detail['current'], 'can_edit': detail['can_edit']}

    def edit(self, memory, project, **changes):
        body = {'project': project, 'id': memory['id'], 'expected_revision': memory['revision'],
                'operation_id': str(uuid.uuid4()), 'content': memory['content'],
                'title': memory['title'], 'kind': memory['kind'],
                'confirmed': memory['state'] == 'confirmed'}
        body.update(changes)
        return body

    def test_owner_and_write_member_edit_read_member_and_strangers_cannot(self):
        ana = self.google_session('ana@example.test')
        mine = self.first(ana, 'notas')
        self.assertTrue(mine['can_edit'])
        status, saved = self.post(ana, self.edit(mine, 'notas', content='ana edited', kind='decision',
                                                 title='Nuevo', confirmed=True), self.csrf(ana))
        self.assertEqual(status, 200, saved)
        self.assertEqual((saved['revision'], saved['content'], saved['kind'], saved['title'],
                          saved['state'], saved['project']),
                         (2, 'ana edited', 'decision', 'Nuevo', 'confirmed', 'notas'))
        # A read member sees the memory but cannot edit it.
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'read'})
        bob = self.google_session('bob@example.test')
        shared = self.first(bob, 'ana/notas')
        self.assertFalse(shared['can_edit'])
        denied = self.post(bob, self.edit(shared, 'ana/notas', content='bob was here'), self.csrf(bob))
        self.assertEqual(denied[0], 403)
        # Unshared and missing projects of others answer the same.
        diario = self.first(ana, 'diario')
        self.assertEqual(self.post(bob, self.edit(diario, 'ana/diario', content='x'), self.csrf(bob)), denied)
        self.assertEqual(self.post(bob, self.edit(diario, 'ana/nada', content='x'), self.csrf(bob)), denied)
        self.assertEqual(self.first(ana, 'diario')['revision'], 1)
        # Promoted to write, the same session may edit.
        self.tool(ANA, 'share', {'project': 'notas', 'email': 'bob@example.test', 'role': 'write'})
        shared = self.first(bob, 'ana/notas')
        self.assertTrue(shared['can_edit'])
        status, saved = self.post(bob, self.edit(shared, 'ana/notas', content='bob edited'), self.csrf(bob))
        self.assertEqual((status, saved['revision'], saved['project']), (200, 3, 'ana/notas'))
        self.assertEqual(self.first(ana, 'notas')['content'], 'bob edited')

    def test_stale_revision_conflicts_and_keeps_the_newer_version(self):
        ana = self.google_session('ana@example.test')
        token = self.csrf(ana)
        old = self.first(ana, 'notas')
        self.assertEqual(self.post(ana, self.edit(old, 'notas', content='first'), token)[0], 200)
        status, body = self.post(ana, self.edit(old, 'notas', content='second'), token)
        self.assertEqual(status, 409, body)
        current = self.first(ana, 'notas')
        self.assertEqual((current['revision'], current['content']), (2, 'first'))
        # Choosing "keep mine" resubmits against the revision now current.
        status, saved = self.post(ana, self.edit(current, 'notas', content='second'), token)
        self.assertEqual((status, saved['revision']), (200, 3))
        # Retrying the same operation is idempotent.
        body = self.edit(self.first(ana, 'notas'), 'notas', content='third')
        self.assertEqual(self.post(ana, body, token), self.post(ana, body, token))
        self.assertEqual(self.first(ana, 'notas')['revision'], 4)

    def test_writes_need_origin_token_and_json(self):
        ana = self.google_session('ana@example.test')
        token = self.csrf(ana)
        self.assertRegex(token, r'^[0-9a-f]{64}$')
        body = self.edit(self.first(ana, 'notas'), 'notas', content='forged')
        for args in ({'csrf': None}, {'csrf': 'f' * 64}, {'csrf': token, 'origin': None},
                     {'csrf': token, 'origin': 'https://evil.example.test'}):
            self.assertEqual(self.post(ana, body, **args)[0], 403, args)
        self.assertEqual(self.post(ana, body, token, content_type='text/plain')[0], 415)
        self.assertEqual(self.post('', body, token)[0], 401)
        # Another user's token does not fit this session.
        other = self.csrf(self.google_session('bob@example.test'))
        self.assertNotEqual(other, token)
        self.assertEqual(self.post(ana, body, other)[0], 403)
        self.assertEqual(self.first(ana, 'notas')['revision'], 1)

    def test_untitled_memories_edit_and_titles_are_kept(self):
        self.tool(ANA, 'remember', {'project': 'notas', 'content': 'sin título', 'source': 'test',
                                    'agent': 'codex', 'operation_id': str(uuid.uuid4())})
        ana = self.google_session('ana@example.test')
        token = self.csrf(ana)
        [item] = [m for m in self.get(ana, 'memories', project='notas')[1]['items'] if not m['title']]
        memory = self.get(ana, 'memory', project='notas', id=item['id'])[1]['current']
        status, saved = self.post(ana, self.edit(memory, 'notas', content='editada sin título'), token)
        self.assertEqual((status, saved['title'], saved['revision']), (200, '', 2), saved)
        # An empty title on a titled memory keeps the title.
        titled = self.first(ana, 'diario')
        status, saved = self.post(ana, self.edit(titled, 'diario', title='  ', content='otra'), token)
        self.assertEqual((status, saved['title']), (200, titled['title']))

    def test_read_key_stays_read_only(self):
        cookie = self.read_key_session()
        self.assertIsNone(self.csrf(cookie))
        memory = self.first(cookie, 'secreto')
        self.assertFalse(memory['can_edit'])
        self.assertEqual(self.post(cookie, self.edit(memory, 'secreto', content='x'), 'f' * 64)[0], 403)
        self.assertEqual(self.first(cookie, 'secreto')['revision'], 1)

    def test_store_rules_still_apply(self):
        ana = self.google_session('ana@example.test')
        token = self.csrf(ana)
        memory = self.first(ana, 'notas')
        status, body = self.post(ana, self.edit(memory, 'notas', content='password=hunter2hunter2'), token)
        self.assertEqual((status, body['error']), (400, 'secret rejected; nothing saved'))
        self.assertEqual(self.post(ana, self.edit(memory, 'notas', kind='idea'), token)[0], 400)
        self.assertEqual(self.post(ana, self.edit(memory, 'notas', extra=1), token)[0], 400)
        # A topic may be set once, then stays stable.
        status, saved = self.post(ana, self.edit(memory, 'notas', topic='notas-tema'), token)
        self.assertEqual((status, saved['topic']), (200, 'notas-tema'))
        status, body = self.post(ana, self.edit(saved, 'notas', topic='otro'), token)
        self.assertEqual(status, 400)
        self.assertIn('topic is stable', body['error'])
        self.assertEqual(self.first(ana, 'notas')['revision'], 2)


# Run only the edit tests here; the inherited ones run in panel_google_protocol.py.
for _name in dir(base.PanelGoogle):
    if _name.startswith('test_'):
        setattr(Edit, _name, None)


if __name__ == '__main__':
    unittest.main()
