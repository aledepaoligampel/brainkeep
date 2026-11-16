"""Agent activity in the panel: agents that saved revisions since a given moment, only
in projects the session can see (panel edits and archived memories excluded). Reuses
the simulated-Google harness; all identities and memories are synthetic."""
import datetime
import json
import time
import unittest
import uuid

import panel_google_protocol as base

ANA = base.ANA
ADMIN = base.ADMIN


def iso(delta=datetime.timedelta()):
    moment = datetime.datetime.now(datetime.timezone.utc) + delta
    return moment.strftime('%Y-%m-%dT%H:%M:%S.') + f'{moment.microsecond // 1000:03d}Z'


class Activity(base.PanelGoogle):
    def save(self, token, project, agent, topic=None):
        args = {'project': project, 'content': f'{agent} en {project}', 'source': 'test', 'agent': agent,
                'operation_id': str(uuid.uuid4())}
        if topic:
            args['topic'] = topic
        return self.tool(token, 'remember', args)

    def agents(self, cookie, **params):
        status, body = self.get(cookie, 'activity', **params)
        self.assertEqual(status, 200, body)
        return {a['agent']: a['revisions'] for a in body['agents']}

    def test_agents_active_in_visible_projects_only(self):
        time.sleep(0.01)  # setUp's writes stay before the window
        start = iso()
        time.sleep(0.01)
        self.save(ANA, 'notas', 'codex')
        self.save(ANA, 'notas', 'codex')
        self.save(ANA, 'diario', 'claude-code')
        self.save(ADMIN, 'secreto', 'antigravity')
        ana = self.google_session('ana@example.test')
        # setUp saved with agent "test"; everything here is recent.
        self.assertEqual(self.agents(ana, since=start), {'codex': 2, 'claude-code': 1})
        self.assertEqual(self.agents(ana)['test'], 2)
        # Sharing the admin project makes its agent visible; revoking hides it again.
        self.tool(ADMIN, 'share', {'project': 'secreto', 'email': 'ana@example.test', 'role': 'read'})
        self.assertEqual(self.agents(ana, since=start), {'codex': 2, 'claude-code': 1, 'antigravity': 1})
        self.tool(ADMIN, 'unshare', {'project': 'secreto', 'email': 'ana@example.test'})
        self.assertNotIn('antigravity', self.agents(ana, since=start))
        # Panel edits are people, not agents; archived memories leave the count.
        token = self.get(ana, 'session')[1]['csrf']
        [memory] = [m for m in self.get(ana, 'memories', project='diario')[1]['items'] if m['agent'] == 'claude-code']
        headers = {'Cookie': ana, 'Content-Type': 'application/json', 'Origin': base.ORIGIN, 'X-CSRF-Token': token}
        body = {'project': 'diario', 'id': memory['id'], 'expected_revision': 1, 'content': 'editado',
                'operation_id': str(uuid.uuid4())}
        self.assertEqual(self.http('POST', '/panel/api/correct', json.dumps(body), headers)[0], 200)
        self.assertNotIn('panel', self.agents(ana, since=start))
        body = {'project': 'diario', 'id': memory['id'], 'archived': True}
        self.assertEqual(self.http('POST', '/panel/api/archive', json.dumps(body), headers)[0], 200)
        self.assertEqual(self.agents(ana, since=start), {'codex': 2})
        # Later moments see nothing; the admin read key sees only the admin's projects.
        self.assertEqual(self.agents(ana, since=iso(datetime.timedelta(minutes=1))), {})
        self.assertEqual(self.agents(self.read_key_session(), since=start), {'antigravity': 1})

    def test_since_is_validated(self):
        ana = self.google_session('ana@example.test')
        for bad in ('ayer', '2026-11-16', '2026-11-16T03:00:00Z', iso(datetime.timedelta(days=-8)), 'x' * 50):
            self.assertEqual(self.get(ana, 'activity', since=bad)[0], 400, bad)
        self.assertEqual(self.get(ana, 'activity', since=iso(), other='1')[0], 400)
        self.assertEqual(self.get('', 'activity')[0], 401)


# Run only the activity tests here; the inherited ones run in panel_google_protocol.py.
for _name in dir(base.PanelGoogle):
    if _name.startswith('test_'):
        setattr(Activity, _name, None)


if __name__ == '__main__':
    unittest.main()
