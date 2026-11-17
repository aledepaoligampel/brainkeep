// Invitation text. With Google sign-in the person connects every agent with OAuth from
// "Connect agents", so the message carries no token; otherwise the token is the only way in.
export function inviteMessage(t, origin, { google, email, token }) {
  const panel = origin + '/panel/';
  if (google && email) return t('inviteMessageConnect', email, panel, panel + '#/connect');
  return t('inviteMessage', panel, origin + '/mcp', token) + t('inviteMessageToken');
}
