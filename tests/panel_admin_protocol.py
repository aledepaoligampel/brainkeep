"""Panel administration (panel-admin S1–S3): the admin key, the users API and personal
tokens, with and without Google configured. Reuses the simulated-Google harness; all
identities, credentials and memories are synthetic."""
import json
import re
import subprocess
import unittest

import panel_google_protocol as base

ORIGIN = base.ORIGIN
ADMIN_KEY = 'e' * 64
PERSONAL = re.compile(r'^sbp_[0-9a-f]{64}$')


class Admin(base.PanelGoogle):

    def setUp(self):
        self.panel_file = {'read': base.READ, 'session_key': base.KEY, 'admin': ADMIN_KEY}
        super().setUp()

    def write_private(self, name, value):
        if name == 'panel.json' and hasattr(self, 'panel_file'):
            value = self.panel_file
        super().write_private(name, value)

    # Sessions -------------------------------------------------------------

    def login(self, token):
        status, body, headers = self.http('POST', '/panel/api/login', json.dumps({'token': token}),
                                          {'Origin': ORIGIN, 'Content-Type': 'application/json'})
        cookies = [c.split(';')[0] for c in self.set_cookies(headers) if c.startswith('brainkeep_panel=')]
        return status, (cookies[0] if cookies else None)

    def admin_session(self):
        status, cookie = self.login(ADMIN_KEY)
        self.assertEqual(status, 200)
        return cookie

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

    def handles(self, cookie):
        status, body = self.get(cookie, 'users')
        self.assertEqual(status, 200, body)
        return {u['handle']: u for u in body['users']}

    def mcp_status(self, token):
        rpc = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call',
               'params': {'name': 'projects', 'arguments': {}}}
        return self.http('POST', '/mcp', json.dumps(rpc), {
            'Accept': 'application/json, text/event-stream', 'Content-Type': 'application/json',
            'MCP-Protocol-Version': '2025-03-26', 'Authorization': 'Bearer ' + token})[0]

    def invite(self, cookie, email, handle):
        status, body = self.post(cookie, 'invite', {'email': email, 'handle': handle})
        self.assertEqual(status, 200, body)
        self.assertRegex(body['token'], PERSONAL)
        return body['token']

    # Tests -----------------------------------------------------------------

    def test_admin_key_opens_a_writable_admin_session(self):
        self.assertEqual(self.get('', 'login')[1]['google'], self.oauth)
        admin = self.admin_session()
        status, session = self.get(admin, 'session')
        self.assertEqual((status, session['user'], session['admin']), (200, 'owner', True))
        self.assertRegex(session['csrf'], r'^[0-9a-f]{64}$')
        # It edits like the admin: a new revision of the admin's own memory.
        [item] = self.get(admin, 'memories', project='secreto')[1]['items']
        memory = self.get(admin, 'memory', project='secreto', id=item['id'])[1]
        self.assertTrue(memory['can_edit'])
        current = memory['current']
        status, saved = self.post(admin, 'correct', {
            'project': 'secreto', 'id': current['id'], 'expected_revision': current['revision'],
            'operation_id': 'op-admin-key-edit', 'content': 'edited with the admin key',
            'title': current['title'], 'kind': current['kind'], 'confirmed': False})
        self.assertEqual((status, saved['revision']), (200, 2), saved)
        # The read key still acts as the admin but never writes and has no admin views.
        read = self.read_key_session()
        session = self.get(read, 'session')[1]
        self.assertEqual((session['user'], session['admin'], session['csrf']), (None, False, None))
        self.assertEqual(self.get(read, 'users')[0], 403)
        self.assertEqual(self.post(read, 'invite', {'email': 'x@example.test', 'handle': 'x'}, 'f' * 64)[0], 403)
        # Logging out ends the admin-key session on the server.
        status, _, _ = self.http('POST', '/panel/api/logout', '{}', {
            'Origin': ORIGIN, 'Content-Type': 'application/json', 'Cookie': admin})
        self.assertEqual(status, 200)
        self.assertEqual(self.get(admin, 'users')[0], 401)

    def test_admin_lists_invites_disables_and_enables(self):
        admin = self.admin_session()
        users = self.handles(admin)
        self.assertEqual(sorted(users), ['ana', 'bob', 'owner'])
        self.assertEqual((users['owner']['admin'], users['ana']['admin'], users['ana']['disabled']),
                         (True, False, False))
        self.assertEqual(users['ana']['email'], 'ana@example.test')
        for field in ('name', 'picture', 'created_at', 'has_token', 'signed_in'):
            self.assertIn(field, users['ana'])
        token = self.invite(admin, 'Carla@Example.test', 'carla')
        users = self.handles(admin)
        self.assertEqual((users['carla']['email'], users['carla']['has_token'], users['carla']['signed_in']),
                         ('carla@example.test', True, False))
        # The personal token works on /mcp and opens the panel as Carla.
        self.assertEqual(self.mcp_status(token), 200)
        self.remember(token, 'propio', 'carla memory')
        self.assertTrue(self.handles(admin)['carla']['signed_in'])
        status, carla = self.login(token)
        self.assertEqual(status, 200)
        session = self.get(carla, 'session')[1]
        self.assertEqual((session['user'], session['admin']), ('carla', False))
        self.assertEqual(self.view(carla)['listed'], ['carla memory'])
        # Clear errors for reused emails or handles and for the admin.
        for body, message in (({'email': 'carla@example.test', 'handle': 'carla2'}, 'email already belongs to another user'),
                              ({'email': 'otra@example.test', 'handle': 'carla'}, 'handle already belongs to another email'),
                              ({'email': 'otra@example.test', 'handle': 'Bad Handle'}, 'invalid email or handle'),
                              ({'email': 'no-at-sign', 'handle': 'otra'}, 'invalid email or handle')):
            status, reply = self.post(admin, 'invite', body)
            self.assertEqual((status, reply['error']), (400, message), body)
        status, reply = self.post(admin, 'disable', {'handle': 'owner'})
        self.assertEqual((status, reply['error']), (400, 'user not found or is the admin'))
        # Disabling cuts the token, the panel session and the user's streams.
        status, reply = self.post(admin, 'disable', {'handle': 'carla'})
        self.assertEqual((status, reply['disabled']), (200, True))
        self.assertEqual(self.mcp_status(token), 401)
        self.assertEqual(self.get(carla, 'status')[0], 401)
        self.assertEqual(self.get(carla, 'events')[0], 401)
        self.assertEqual(self.login(token)[0], 401)
        self.assertEqual(self.handles(admin)['carla']['has_token'], False)
        # Enabling issues a new token; the old one and old sessions stay dead.
        status, reply = self.post(admin, 'enable', {'handle': 'carla'})
        self.assertEqual(status, 200, reply)
        self.assertRegex(reply['token'], PERSONAL)
        self.assertEqual(self.mcp_status(token), 401)
        self.assertEqual(self.get(carla, 'status')[0], 401)
        self.assertEqual(self.mcp_status(reply['token']), 200)
        self.assertEqual(self.handles(admin)['carla']['disabled'], False)
        status, reply = self.post(admin, 'enable', {'handle': 'ana'})
        self.assertEqual((status, reply['error']), (400, 'user not found, not disabled or is the admin'))

    def test_regenerating_a_token_revokes_the_previous_one(self):
        admin = self.admin_session()
        first = self.invite(admin, 'dani@example.test', 'dani')
        status, dani = self.login(first)
        self.assertEqual((status, self.get(dani, 'status')[0]), (200, 200))
        status, reply = self.post(admin, 'token', {'handle': 'dani'})
        self.assertEqual(status, 200, reply)
        second = reply['token']
        self.assertNotEqual(first, second)
        self.assertEqual(self.mcp_status(first), 401)
        self.assertEqual(self.login(first)[0], 401)
        # Panel sessions opened with the old token end with it.
        self.assertEqual(self.get(dani, 'status')[0], 401)
        self.assertEqual(self.mcp_status(second), 200)
        # Users invited by console get a token from the panel too; the admin does not.
        status, reply = self.post(admin, 'token', {'handle': 'ana'})
        self.assertEqual(status, 200, reply)
        self.assertEqual(self.mcp_status(reply['token']), 200)
        for handle in ('owner', 'nadie'):
            status, reply = self.post(admin, 'token', {'handle': handle})
            self.assertEqual((status, reply['error']), (400, 'user not found, disabled or is the admin'), handle)
        # Only hashes are stored: no token appears in any database file.
        data = b''.join(p.read_bytes() for p in (self.root / 'data').iterdir() if p.is_file())
        for token in (first, second):
            self.assertNotIn(token.encode(), data)
            self.assertNotIn(token[4:].encode(), data)

    def test_users_issue_their_own_token(self):
        admin = self.admin_session()
        first = self.invite(admin, 'lia@example.test', 'lia')
        # A session opened with the token cannot rotate it: the session would end with the
        # old token before the new one could be copied.
        by_token = self.login(first)[1]
        status, reply = self.post(by_token, 'my_token', {})
        self.assertEqual((status, reply['error']), (400, 'sign in with Google to issue a new token'))
        self.assertEqual(self.mcp_status(first), 200)
        if not self.oauth:
            return
        # From a Google session: a new token, the old one stops working, the session stays.
        lia = self.google_session('lia@example.test')
        status, reply = self.post(lia, 'my_token', {})
        self.assertEqual(status, 200, reply)
        second = reply['token']
        self.assertRegex(second, PERSONAL)
        self.assertNotEqual(first, second)
        self.assertEqual((self.mcp_status(first), self.mcp_status(second)), (401, 200))
        self.assertEqual(self.get(lia, 'status')[0], 200)
        # Once a minute at most, so a stolen session cannot churn tokens.
        status, reply = self.post(lia, 'my_token', {})
        self.assertEqual((status, reply['error']), (400, 'token was just issued; try again in a minute'))
        self.assertEqual(self.mcp_status(second), 200)
        # Unknown fields, the read key, no session, no CSRF token or another origin: no token.
        self.assertEqual(self.post(lia, 'my_token', {'handle': 'ana'})[0], 400)
        self.assertEqual(self.post(self.read_key_session(), 'my_token', {})[0], 403)
        self.assertEqual(self.post('', 'my_token', {}, csrf=None)[0], 401)
        self.assertEqual(self.post(lia, 'my_token', {}, csrf=None)[0], 403)
        self.assertEqual(self.post(lia, 'my_token', {}, origin='https://evil.example.test')[0], 403)
        # The admin connects with OAuth or fixed tokens, not a personal one.
        status, reply = self.post(admin, 'my_token', {})
        self.assertEqual((status, reply['error']), (400, 'the admin has no personal token'))

    def test_admin_deletes_only_users_without_data(self):
        admin = self.admin_session()
        tomi = self.invite(admin, 'tomi@example.test', 'tomi')
        rosa = self.invite(admin, 'rosa@example.test', 'rosa')
        ivo = self.invite(admin, 'ivo@example.test', 'ivo')
        self.remember(rosa, 'propio', 'rosa memory')
        # Ivo owns nothing but wrote into a project shared with him.
        self.tool(base.ADMIN, 'share', {'project': 'secreto', 'email': 'ivo@example.test', 'role': 'write'})
        self.remember(ivo, 'owner/secreto', 'ivo wrote here')
        self.tool(base.ADMIN, 'share', {'project': 'secreto', 'email': 'tomi@example.test', 'role': 'read'})
        # Pia saved, shared, archived and purged: her project still has members and a purge
        # record, so deleting her would orphan rows (and stop the next start).
        pia = self.invite(admin, 'pia@example.test', 'pia')
        saved = self.tool(pia, 'remember', {'project': 'borrador', 'content': 'pia memory', 'kind': 'note',
                                            'title': 'pia', 'source': 'test', 'agent': 'test',
                                            'operation_id': 'pia-op-0001'})
        self.tool(pia, 'share', {'project': 'borrador', 'email': 'tomi@example.test', 'role': 'read'})
        pia_panel = self.login(pia)[1]
        target = {'project': 'borrador', 'id': saved['id']}
        self.assertEqual(self.post(pia_panel, 'archive', {**target, 'archived': True})[0], 200)
        self.assertEqual(self.post(pia_panel, 'purge', {**target, 'confirm': 'purgar'})[0], 200)
        users = self.handles(admin)
        self.assertEqual({h: users[h]['has_data'] for h in ('tomi', 'rosa', 'ivo', 'pia')},
                         {'tomi': False, 'rosa': True, 'ivo': True, 'pia': True})
        for handle in ('rosa', 'ivo', 'pia'):
            status, reply = self.post(admin, 'delete', {'handle': handle})
            self.assertEqual((status, reply['error']), (400, 'user has memories or edits; disable instead'), handle)
        for handle in ('owner', 'nadie'):
            status, reply = self.post(admin, 'delete', {'handle': handle})
            self.assertEqual((status, reply['error']), (400, 'user not found or is the admin'), handle)
        status, reply = self.post(admin, 'delete', {'handle': 'tomi'})
        self.assertEqual((status, reply), (200, {'handle': 'tomi', 'deleted': True}))
        self.assertEqual(self.mcp_status(tomi), 401)
        self.assertNotIn('tomi', self.handles(admin))
        self.assertEqual((self.mcp_status(rosa), self.mcp_status(ivo)), (200, 200))
        # The email and handle are free again.
        self.assertEqual(self.mcp_status(self.invite(admin, 'tomi@example.test', 'tomi')), 200)
        self.assertFalse(self.handles(admin)['tomi']['has_data'])

    def test_only_the_admin_reaches_user_administration(self):
        admin = self.admin_session()
        ana_token = self.invite(admin, 'eva@example.test', 'eva')
        status, eva = self.login(ana_token)
        self.assertEqual(status, 200)
        before = self.handles(admin)
        # A non-admin session gets 404 as if the views did not exist.
        self.assertEqual(self.get(eva, 'users')[0], 404)
        for view, body in (('invite', {'email': 'x@example.test', 'handle': 'x'}), ('disable', {'handle': 'ana'}),
                           ('enable', {'handle': 'ana'}), ('token', {'handle': 'ana'}),
                           ('delete', {'handle': 'ana'})):
            self.assertEqual(self.post(eva, view, body)[0], 404, view)
            # No session, no CSRF token, a foreign token or another origin: no effect.
            self.assertEqual(self.post('', view, body, csrf=None)[0], 401, view)
            self.assertEqual(self.post(admin, view, body, csrf=None)[0], 403, view)
            self.assertEqual(self.post(admin, view, body, csrf='f' * 64)[0], 403, view)
            self.assertEqual(self.post(admin, view, body, origin='https://evil.example.test')[0], 403, view)
            self.assertEqual(self.post(admin, view, body, origin=None)[0], 403, view)
        self.assertEqual(self.handles(admin), before)
        self.assertEqual(self.mcp_status(ana_token), 200)
        # Agents gain no administration tools.
        rpc = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/list', 'params': {}}
        status, body, _ = self.http('POST', '/mcp', json.dumps(rpc), {
            'Accept': 'application/json, text/event-stream', 'Content-Type': 'application/json',
            'MCP-Protocol-Version': '2025-03-26', 'Authorization': 'Bearer ' + base.ADMIN})
        names = {t['name'] for t in json.loads(body)['result']['tools']}
        self.assertFalse(names & {'invite', 'disable', 'enable', 'token', 'users'}, names)

    def test_invites_never_touch_the_admin_or_rotate_tokens_silently(self):
        admin = self.admin_session()
        before = self.handles(admin)['owner']['email']
        # The UI suggests the handle from the email: `owner@…` proposes the admin's handle.
        status, reply = self.post(admin, 'invite', {'email': 'owner@stranger.test', 'handle': 'owner'})
        self.assertEqual(status, 400, reply)
        self.assertIn(reply['error'], ('email or handle belongs to the admin', 'handle already belongs to another email'))
        self.assertEqual(self.handles(admin)['owner']['email'], before)
        # Re-inviting an enabled user does not issue a new token behind their back.
        token = self.invite(admin, 'fran@example.test', 'fran')
        status, reply = self.post(admin, 'invite', {'email': 'fran@example.test', 'handle': 'fran'})
        self.assertEqual((status, reply['error']), (400, 'user already exists and is enabled'))
        self.assertEqual(self.mcp_status(token), 200)
        # A disabled user can be invited again: that enables them with a new token.
        self.post(admin, 'disable', {'handle': 'fran'})
        again = self.invite(admin, 'fran@example.test', 'fran')
        self.assertEqual((self.mcp_status(token), self.mcp_status(again)), (401, 200))

    def test_login_attempts_are_throttled_for_every_kind_of_credential(self):
        for i in range(10):
            self.assertEqual(self.login('sbp_' + f'{i:064x}')[0], 401)
        self.assertEqual(self.login(ADMIN_KEY)[0], 429)


class AdminWithoutGoogle(Admin):
    oauth = False


class AdminWithoutEmail(Admin):
    """A fresh installation: the admin user `owner` has no email yet."""
    oauth = False

    def users(self, *args):
        if args[:2] == ('invite', 'owner@example.test'):
            return
        super().users(*args)


class AdminKeyValidation(unittest.TestCase):
    """The daemon refuses an admin key that is weak, malformed or reused."""

    def run_with(self, panel):
        import tempfile
        from pathlib import Path
        with tempfile.TemporaryDirectory(prefix='brain-panel-admin-') as temp:
            root = Path(temp)
            for name, value in (('auth.json', {'admin-codex': base.ADMIN}), ('panel.json', panel)):
                (root / name).write_text(json.dumps(value))
                (root / name).chmod(0o600)
            out = subprocess.run([str(base.BINARY), '--data-dir', str(root / 'data'), 'serve',
                                  '--http-port', str(base.free_port()), '--http-host', base.HOST,
                                  '--auth-file', str(root / 'auth.json'),
                                  '--panel-auth-file', str(root / 'panel.json')],
                                 capture_output=True, text=True, timeout=10)
            return out.returncode, out.stderr

    def test_invalid_admin_keys_stop_startup(self):
        good = {'read': base.READ, 'session_key': base.KEY}
        for admin in (base.READ, base.KEY, base.ADMIN, '0' * 64, 'e' * 63, 'g' * 64, base.KEY.upper()):
            code, err = self.run_with({**good, 'admin': admin})
            self.assertNotEqual(code, 0, admin)
            self.assertIn('panel', err, admin)
        code, err = self.run_with({**good, 'admin': ADMIN_KEY, 'other': 'x'})
        self.assertNotEqual(code, 0)


# Run only the admin tests here; the inherited ones run in panel_google_protocol.py.
for _name in dir(base.PanelGoogle):
    if _name.startswith('test_') and _name not in Admin.__dict__:
        setattr(Admin, _name, None)
for _name in dir(Admin):
    if _name.startswith('test_') and _name != 'test_invites_never_touch_the_admin_or_rotate_tokens_silently':
        setattr(AdminWithoutEmail, _name, None)


if __name__ == '__main__':
    unittest.main()
