// Pure rendering rules of the panel (S4, S5). Run: node tests/panel_render.mjs
import assert from 'node:assert/strict';
import { parseMarkdown, lineDiff, safeHref, author } from '../panel-ui/src/lib/markdown.js';
import { inviteMessage } from '../panel-ui/src/lib/invite.js';
import { connectView } from '../panel-ui/src/lib/connect.js';
import { I18N } from '../panel-ui/src/lib/i18n.js';

const ALLOWED = new Set(['p', 'h', 'ul', 'ol', 'li', 'pre', 'text', 'strong', 'em', 'code', 'a', 'br']);
function walk(node, visit) {
  visit(node);
  for (const child of node.children || []) walk(child, visit);
}
function text(nodes) {
  let out = '';
  for (const n of nodes) walk(n, (x) => { if (x.t === 'text' || x.t === 'code' || x.t === 'pre') out += x.v ?? ''; });
  return out;
}

// Supported subset.
const doc = parseMarkdown('# Título\n\nTexto **fuerte** y *suave* con `code` y [enlace](https://example.com/a?b=1).\n\n- uno\n- dos\n\n1. a\n2. b\n\n```\n<b>x</b>\n```');
assert.deepEqual(doc.map((n) => n.t), ['h', 'p', 'ul', 'ol', 'pre']);
const p = doc[1].children;
assert.ok(p.some((n) => n.t === 'strong' && n.children[0].v === 'fuerte'));
assert.ok(p.some((n) => n.t === 'em' && n.children[0].v === 'suave'));
assert.ok(p.some((n) => n.t === 'code' && n.v === 'code'));
assert.ok(p.some((n) => n.t === 'a' && n.href === 'https://example.com/a?b=1'));
assert.equal(doc[2].children.length, 2);
assert.equal(doc[4].v, '<b>x</b>');

// Hostile corpus: everything stays inert text; only http(s) links survive.
const hostile = [
  '<script>alert(1)</script>',
  '<img src=x onerror=alert(2)>',
  '[x](javascript:alert(3))',
  '[x](JaVaScRiPt:alert(4))',
  '[x]( javascript:alert(5))',
  '[x](data:text/html,<script>alert(6)</script>)',
  '[x](vbscript:msgbox)',
  '[x](//evil.example)',
  '<svg onload=alert(7)>',
  '<a href="javascript:alert(8)">a</a>',
  '**<iframe src=x>**',
  '`<script>`',
].join('\n\n');
const nodes = parseMarkdown(hostile);
for (const node of nodes) {
  walk(node, (n) => {
    assert.ok(ALLOWED.has(n.t), `unexpected node ${n.t}`);
    assert.equal(Object.keys(n).filter((k) => !['t', 'v', 'children', 'href', 'level'].includes(k)).length, 0);
    if (n.t === 'a') assert.match(n.href, /^https?:\/\//);
  });
}
const flat = text(nodes);
for (const literal of ['<script>alert(1)</script>', '<img src=x onerror=alert(2)>', '<svg onload=alert(7)>', '<iframe src=x>']) {
  assert.ok(flat.includes(literal), literal);
}
for (const bad of ['javascript:x', ' javascript:x', 'data:text/html,x', '//evil', 'vbscript:x', 'http:/x', 'https://exa mple.com']) {
  assert.equal(safeHref(bad), null, bad);
}
assert.equal(safeHref('https://example.com/x'), 'https://example.com/x');

// Bounded input never throws and always terminates.
parseMarkdown('*'.repeat(20000));
parseMarkdown('['.repeat(5000) + '](' .repeat(5000));
for (const sep of ['\u2028', '\u2029', '\u0085', '\v', '\f']) {
  const out = parseMarkdown('# a' + sep + 'b\n#' + sep + '\n```' + sep);
  assert.ok(out.length > 0 && out.length < 10, JSON.stringify(sep));
}

// Line diff.
assert.deepEqual(lineDiff('a\nb\nc', 'a\nx\nc'), [
  { op: ' ', t: 'a' }, { op: '-', t: 'b' }, { op: '+', t: 'x' }, { op: ' ', t: 'c' },
]);
assert.deepEqual(lineDiff('', 'n'), [{ op: '-', t: '' }, { op: '+', t: 'n' }]);
assert.deepEqual(lineDiff('same', 'same'), [{ op: ' ', t: 'same' }]);
const big = Array.from({ length: 3000 }, (_, i) => `l${i}`).join('\n');
const d = lineDiff(big, big.replace('l1500', 'L1500'));
assert.equal(d.filter((x) => x.op !== ' ').length, 2);

// Verified author: principal, or a dash for revisions saved before schema v3.
assert.equal(author({ principal: 'ana', agent: 'codex' }), 'ana');
assert.equal(author({ principal: null, agent: 'codex' }), '—');
assert.equal(author({ agent: 'codex' }), '—');

// Invitations (conectar-agentes S1): with Google the text carries no token.
const TOKEN = 'sbp_' + 'a'.repeat(64);
for (const lang of ['es', 'en']) {
  const t = (key, ...args) => I18N[lang][key].replace(/\{(\d)\}/g, (_, i) => args[i]);
  const google = inviteMessage(t, 'https://m.example.test', { google: true, email: 'ana@example.test', token: TOKEN });
  assert.ok(!google.includes('sbp_'), google);
  assert.ok(google.includes('ana@example.test') && google.includes('https://m.example.test/panel/#/connect'), google);
  for (const fallback of [{ google: false, email: 'ana@example.test' }, { google: true, email: null }]) {
    const text = inviteMessage(t, 'https://m.example.test', { ...fallback, token: TOKEN });
    assert.ok(text.includes(TOKEN) && text.includes('https://m.example.test/mcp'), text);
  }
}

// "Connect agents" without Google (despliegue-railway S6). A server deployed without a
// Google client has no OAuth endpoints: the page must not send anyone there.
const CLIENTS = ['claude-code', 'codex', 'antigravity', 'claude-ai', 'chatgpt'];
const URL_MCP = 'https://m.example.test/mcp';
const SESSIONS = {
  admin: { user: 'admin', admin: true },
  user: { user: 'ana', admin: false },
  read: { user: null, admin: false },
};
for (const lang of ['es', 'en']) {
  const t = (key, ...args) => {
    const v = I18N[lang][key];
    assert.ok(v !== undefined, `${lang}.${key} missing`);
    return typeof v === 'string' ? v.replace(/\{(\d)\}/g, (_, i) => args[i]) : v;
  };
  const steps = (view, client) => t(view.stepsKey)[client];
  const words = (view, client) => [t(view.intro), view.note ? t(view.note) : '', ...steps(view, client)].join('\n');
  for (const [who, session] of Object.entries(SESSIONS)) {
    for (const client of CLIENTS) {
      // Without Google: token commands are the main steps, nothing mentions authorizing with Google.
      const off = connectView({ client, google: false, session, url: URL_MCP, token: '' });
      assert.equal(off.tokenSection, false, `${who}/${client}`);
      assert.equal(off.canIssue, false, `${who}/${client}: no Google session can issue a token`);
      if (['claude-ai', 'chatgpt'].includes(client)) {
        assert.equal(off.primary, null, client);
        assert.equal(off.note, 'connectNeedsGoogle', client);
      } else {
        assert.ok(off.primary.length > 0 && off.primary.every((b) => b.includes('Authorization') && b.includes(URL_MCP)), client);
        assert.ok(off.primary.every((b) => b.includes('<token>')), client);
        assert.equal(off.note, { admin: 'tokenAdminNoGoogle', user: 'tokenUserNoGoogle', read: 'tokenReadKeyNoGoogle' }[who], `${who}/${client}`);
        assert.equal(steps(off, client).length, off.primary.length + 1, client);
        assert.ok(!off.primary.some((b) => b.includes('mcp login')), client);
        assert.ok(!/autoriz|authoriz/i.test(words(off, client)), `${lang}/${who}/${client}: ${words(off, client)}`);
      }
      assert.ok(!/con Google|with Google|tu cuenta de Google|your Google account/.test(t(off.intro)), `${lang} intro`);
      // With Google: unchanged OAuth flow, token as a fallback section.
      const on = connectView({ client, google: true, session, url: URL_MCP, token: '' });
      assert.equal(on.intro, 'connectIntro');
      assert.equal(on.tokenSection, true);
      assert.ok(on.primary.every((b) => !b.includes('Authorization')), client);
      assert.equal(on.canIssue, who === 'user');
    }
  }
  // The admin note points to self-invitation and the Railway guide.
  assert.match(t('tokenAdminNoGoogle'), /railway|Railway/);
  assert.ok(t('tokenAdminNoGoogle').includes(lang === 'es' ? 'Usuarios' : 'Users'));
}
// An issued token replaces the placeholder (Google mode, signed-in user).
const issued = connectView({ client: 'claude-code', google: true, session: SESSIONS.user, url: URL_MCP, token: TOKEN });
assert.ok(issued.token[0].includes('Bearer ' + TOKEN));

console.log('panel_render: ok');
