// What "Connect agents" shows. With Google sign-in configured every client authorizes
// with OAuth and a personal token is a fallback. Without it the server has no OAuth
// endpoints (a template deploy leaves Google optional), so token commands become the
// main steps and the clients that only speak OAuth say they need Google.

// Snippets per client: `oauth` without a token, `token` with one (null: not supported).
export function snippets(id, url, secret) {
  const bearer = 'Bearer ' + secret;
  switch (id) {
    case 'claude-code':
      return {
        oauth: [`claude mcp add --transport http --scope user brainkeep ${url}`],
        token: [`claude mcp add --transport http --scope user brainkeep ${url} --header "Authorization: ${bearer}"`],
      };
    case 'codex':
      return {
        oauth: [`[mcp_servers.brainkeep]\nurl = "${url}"`, 'codex mcp login brainkeep'],
        token: [`[mcp_servers.brainkeep]\nurl = "${url}"\nhttp_headers = { Authorization = "${bearer}" }`],
      };
    case 'antigravity':
      return {
        oauth: [JSON.stringify({ mcpServers: { brainkeep: { serverUrl: url } } }, null, 2)],
        token: [JSON.stringify({ mcpServers: { brainkeep: { serverUrl: url, headers: { Authorization: bearer } } } }, null, 2)],
      };
    default:
      return { oauth: [url], token: null };
  }
}

// `session` is the panel session: { user, admin }. A user session can issue its own
// token only after a Google sign-in, so without Google nobody can issue one here.
export function connectView({ client, google, session, url, token }) {
  const s = snippets(client, url, token || '<token>');
  const admin = Boolean(session?.admin);
  const user = Boolean(session?.user) && !admin;
  if (google) {
    let note = null;
    if (!s.token) note = 'tokenUnsupported';
    else if (!user) note = admin ? 'tokenAdmin' : 'tokenReadKey';
    return { intro: 'connectIntro', stepsKey: 'connectSteps', primary: s.oauth, token: s.token,
      tokenSection: true, canIssue: user, note };
  }
  if (!s.token) {
    return { intro: 'connectIntroToken', stepsKey: 'connectSteps', primary: null, token: null,
      tokenSection: false, canIssue: false, note: 'connectNeedsGoogle' };
  }
  const note = admin ? 'tokenAdminNoGoogle' : user ? 'tokenUserNoGoogle' : 'tokenReadKeyNoGoogle';
  return { intro: 'connectIntroToken', stepsKey: 'connectStepsToken', primary: s.token, token: s.token,
    tokenSection: false, canIssue: false, note };
}
