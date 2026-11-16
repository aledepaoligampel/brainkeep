"""Sharing projects from the panel (panel-admin S5): only the owner shares, with the
same rules as the share/unshare tools; members and strangers get the same answer as for
a missing project. Reuses the simulated-Google harness; all data is synthetic."""
import json
import unittest

import panel_google_protocol as base

ORIGIN = base.ORIGIN


class Share(base.PanelGoogle):
    def csrf(self, cookie):
        status, session = self.get(cookie, 'session')
        self.assertEqual(status, 200, session)
        return session.get('csrf')

    def post(self, cookie, view, body, csrf='auto', origin=ORIGIN):
        headers = {'Cookie': cookie, 'Content-Type': 'application/json'}
        if origin is not None:
            headers['Origin'] = origin
        if csrf == 'auto':
            csrf = self.csrf(cookie) if cookie else None
        if csrf is not None:
            headers['X-CSRF-Token'] = csrf
        status, raw, _ = self.http('POST', '/panel/api/' + view, json.dumps(body), headers)
        return status, json.loads(raw) if raw else None

    def sharing(self, cookie, project):
        return self.get(cookie, 'sharing', project=project)

    def members(self, cookie, project):
        status, body = self.sharing(cookie, project)
        self.assertEqual(status, 200, body)
        return {m['handle']: m['role'] for m in body['members']}

    def test_owner_adds_changes_and_removes_members(self):
        ana = self.google_session('ana@example.test')
        bob = self.google_session('bob@example.test')
        status, body = self.sharing(ana, 'notas')
        self.assertEqual(status, 200, body)
        self.assertEqual((body['project'], body['members']), ('notas', []))
        # Candidates: enabled users other than the owner, by handle and name, never email.
        self.assertEqual(sorted(p['handle'] for p in body['people']), ['bob', 'owner'])
        self.assertNotIn('@', json.dumps(body))
        status, reply = self.post(ana, 'share', {'project': 'notas', 'handle': 'bob', 'role': 'read'})
        self.assertEqual((status, reply['role']), (200, 'read'), reply)
        self.assertEqual(self.members(ana, 'notas'), {'bob': 'read'})
        self.assertEqual(self.view(bob)['projects'], ['ana/notas'])
        [item] = self.get(bob, 'memories', project='ana/notas')[1]['items']
        detail = self.get(bob, 'memory', project='ana/notas', id=item['id'])[1]
        self.assertFalse(detail['can_edit'])
        # Changing the role takes effect on the next request.
        self.assertEqual(self.post(ana, 'share', {'project': 'notas', 'handle': 'bob', 'role': 'write'})[0], 200)
        self.assertEqual(self.members(ana, 'notas'), {'bob': 'write'})
        self.assertTrue(self.get(bob, 'memory', project='ana/notas', id=item['id'])[1]['can_edit'])
        # Removing hides the project at once; what Bob wrote stays with the project.
        status, reply = self.post(ana, 'unshare', {'project': 'notas', 'handle': 'bob'})
        self.assertEqual((status, reply['removed']), (200, True), reply)
        self.assertEqual(self.members(ana, 'notas'), {})
        self.assertEqual(self.view(bob)['projects'], [])
        # The agents see the same membership.
        self.post(ana, 'share', {'project': 'notas', 'handle': 'bob', 'role': 'read'})
        projects = self.tool(base.ANA, 'projects', {})
        [notas] = [p for p in projects['projects'] if p['project'] == 'notas']
        self.assertEqual([(m['handle'], m['role']) for m in notas['members']], [('bob', 'read')])

    def test_clear_errors_follow_the_tool_rules(self):
        ana = self.google_session('ana@example.test')
        cases = (
            ({'project': 'notas', 'handle': 'ana', 'role': 'read'}, 'share', 'no enabled user with that handle'),
            ({'project': 'notas', 'handle': 'nadie', 'role': 'read'}, 'share', 'no enabled user with that handle'),
            ({'project': 'notas', 'handle': 'bob', 'role': 'owner'}, 'share', 'role must be read or write'),
            ({'project': 'notas', 'handle': 'bob'}, 'unshare', 'project is not shared with that user'),
        )
        for body, view, message in cases:
            status, reply = self.post(ana, view, body)
            self.assertEqual((status, reply['error']), (400, message), body)
        self.users('disable', 'bob')
        status, reply = self.post(ana, 'share', {'project': 'notas', 'handle': 'bob', 'role': 'read'})
        self.assertEqual((status, reply['error']), (400, 'no enabled user with that handle'))
        self.assertNotIn('bob', [p['handle'] for p in self.sharing(ana, 'notas')[1]['people']])

    def test_members_strangers_and_missing_projects_look_the_same(self):
        ana = self.google_session('ana@example.test')
        self.post(ana, 'share', {'project': 'notas', 'handle': 'bob', 'role': 'write'})
        bob = self.google_session('bob@example.test')
        body = {'handle': 'owner', 'role': 'read'}
        missing = self.post(bob, 'share', {'project': 'nada', **body})
        self.assertEqual(missing[0], 404)
        for project in ('ana/notas', 'ana/diario', 'ana/nada', 'notas'):
            self.assertEqual(self.post(bob, 'share', {'project': project, **body}), missing, project)
            self.assertEqual(self.post(bob, 'unshare', {'project': project, 'handle': 'ana'}), missing, project)
            self.assertEqual(self.sharing(bob, project)[0], 404, project)
        # An own project without memories cannot be shared, like with the tool.
        self.assertEqual(self.post(ana, 'share', {'project': 'vacio', **body}), missing)
        self.assertEqual(self.members(ana, 'notas'), {'bob': 'write'})

    def test_writes_need_a_user_session_origin_and_token(self):
        ana = self.google_session('ana@example.test')
        body = {'project': 'notas', 'handle': 'bob', 'role': 'read'}
        for args in ({'csrf': None}, {'csrf': 'f' * 64}, {'origin': None}, {'origin': 'https://evil.example.test'}):
            self.assertEqual(self.post(ana, 'share', body, **args)[0], 403, args)
        self.assertEqual(self.post('', 'share', body, csrf=None)[0], 401)
        read = self.read_key_session()
        self.assertEqual(self.sharing(read, 'secreto')[0], 403)
        self.assertEqual(self.post(read, 'share', {**body, 'project': 'secreto'}, csrf='f' * 64)[0], 403)
        self.assertEqual(self.members(ana, 'notas'), {})


# Run only the share tests here; the inherited ones run in panel_google_protocol.py.
for _name in dir(base.PanelGoogle):
    if _name.startswith('test_') and _name not in Share.__dict__:
        setattr(Share, _name, None)


if __name__ == '__main__':
    unittest.main()
