// Shared panel state. Every API value is rendered as text by Svelte; HTML is never parsed.
import { I18N } from './i18n.js';

const prefs = {
  get(key) { try { return window.localStorage.getItem('brainkeep.' + key); } catch { return null; } },
  put(key, value) { try { window.localStorage.setItem('brainkeep.' + key, value); } catch { /* storage unavailable */ } },
};

function initialLang() {
  const saved = prefs.get('lang');
  if (saved && I18N[saved]) return saved;
  return (navigator.language || '').toLowerCase().startsWith('es') ? 'es' : 'en';
}

// `archived=1` switches every view to archived memories; it is a view, not a filter.
export const FILTERS = ['q', 'kind', 'principal', 'agent', 'state', 'topic', 'from', 'to', 'archived'];

export const app = $state({
  lang: initialLang(),
  theme: prefs.get('theme') || 'system',
  session: null, // {expires_at, user, admin, csrf}
  google: null, // whether "Sign in with Google" is configured; null until /login answers
  booted: false,
  bootError: false,
  status: null,
  projects: null, // [{project,total,topics,last_saved_at,kinds}]
  facets: null,
  activity: null, // [{agent, revisions, last_saved_at}] since local midnight
  connection: 'idle', // idle | live | reconnecting | offline
  changed: 0, // bumps when the server reports a visible change
  route: parseRoute(),
});

export function t(key, ...args) {
  const value = I18N[app.lang][key];
  return typeof value === 'string' ? value.replace(/\{(\d)\}/g, (_, n) => String(args[Number(n)])) : value;
}
export function setLang(value) { app.lang = value; prefs.put('lang', value); }
export function setTheme(value) { app.theme = value; prefs.put('theme', value); applyTheme(); }
export function applyTheme() {
  if (app.theme === 'light' || app.theme === 'dark') document.documentElement.dataset.theme = app.theme;
  else delete document.documentElement.dataset.theme;
}

// Routing: #/ · #/p/<project> · #/m/<project>/<id> · #/users, list filters in the query string.
export function parseRoute() {
  const raw = location.hash.replace(/^#/, '') || '/';
  const [path, query = ''] = raw.split('?');
  const parts = path.split('/').filter(Boolean).map((p) => { try { return decodeURIComponent(p); } catch { return ''; } });
  const params = new URLSearchParams(query);
  const filters = Object.fromEntries(FILTERS.map((n) => [n, params.get(n) || '']));
  if (parts[0] === 'users') return { project: '', id: null, filters, scoped: false, users: true };
  if (parts[0] === 'connect') return { project: '', id: null, filters, scoped: false, connect: true };
  if (parts[0] === 'm' && parts[1] && parts[2]) return { project: parts[1], id: parts[2], filters, scoped: params.get('in') === '1' };
  if (parts[0] === 'p' && parts[1]) return { project: parts[1], id: null, filters, scoped: true };
  return { project: '', id: null, filters, scoped: false };
}
function query(filters, extra = {}) {
  const q = new URLSearchParams([...Object.entries(filters), ...Object.entries(extra)].filter(([, v]) => v));
  return q.size ? '?' + q : '';
}
export function listHref(project, filters = app.route.filters) {
  return (project ? '#/p/' + encodeURIComponent(project) : '#/') + query(filters);
}
export function memoryHref(project, id, filters = app.route.filters, scoped = app.route.scoped) {
  return '#/m/' + encodeURIComponent(project) + '/' + encodeURIComponent(id) + query(filters, scoped ? { in: '1' } : {});
}
export function go(target) { if (location.hash !== target) location.hash = target; }
window.addEventListener('hashchange', () => { app.route = parseRoute(); });

// API.
export class ApiError extends Error {
  constructor(status) { super('api ' + status); this.status = status; }
}
export async function api(path, params = {}) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '') q.set(k, v);
  const response = await fetch('/panel/api/' + path + (q.size ? '?' + q : ''), {
    credentials: 'same-origin', headers: { Accept: 'application/json' }, cache: 'no-store',
  });
  if (response.status === 401) { app.session = null; throw new ApiError(401); }
  if (!response.ok) throw new ApiError(response.status);
  return response.json();
}
export function post(path, body) {
  const headers = { 'Content-Type': 'application/json' };
  // Google sessions carry an anti-CSRF token for writes; the read key has none.
  if (app.session?.csrf) headers['X-CSRF-Token'] = app.session.csrf;
  return fetch('/panel/api/' + path, {
    method: 'POST', credentials: 'same-origin', headers, body: JSON.stringify(body),
  });
}

// JSON write with the session's anti-CSRF token: {ok, status, body}, never throws.
export async function send(path, body) {
  try {
    const response = await post(path, body);
    if (response.status === 401) app.session = null;
    const data = await response.json().catch(() => ({}));
    return { ok: response.ok, status: response.status, body: data };
  } catch {
    return { ok: false, status: 0, body: {} };
  }
}

export async function refreshShared(withFacets = true) {
  try {
    // "Today" is the viewer's day: local midnight, sent as UTC.
    const midnight = new Date();
    midnight.setHours(0, 0, 0, 0);
    const [status, projects, facets, activity] = await Promise.all([
      api('status'), api('projects'), withFacets ? api('facets') : null,
      api('activity', { since: midnight.toISOString() }).catch(() => null),
    ]);
    app.status = status;
    app.projects = projects.items;
    if (activity) app.activity = activity.agents;
    if (facets) app.facets = facets;
  } catch (error) {
    if (error.status !== 401) app.status = false;
  }
}

export async function boot() {
  fetch('/panel/api/login', { credentials: 'same-origin', cache: 'no-store' })
    .then((r) => (r.ok ? r.json() : null))
    .then((config) => { app.google = Boolean(config && config.google === true); })
    .catch(() => { app.google = false; });
  try {
    app.session = await api('session');
  } catch (error) {
    app.bootError = error.status !== 401;
    app.booted = true;
    return;
  }
  await refreshShared();
  app.booted = true;
}

export async function logout() {
  await post('logout', {}).catch(() => null);
  app.session = null;
  app.projects = null;
  app.facets = null;
}

// Formatting.
export function when(value) {
  const date = typeof value === 'number' ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const seconds = (date.getTime() - Date.now()) / 1000;
  const units = [['year', 31536000], ['month', 2592000], ['week', 604800], ['day', 86400], ['hour', 3600], ['minute', 60]];
  const rtf = new Intl.RelativeTimeFormat(app.lang, { numeric: 'auto' });
  for (const [unit, size] of units) if (Math.abs(seconds) >= size) return rtf.format(Math.round(seconds / size), unit);
  return rtf.format(Math.round(seconds), 'second');
}
export function stamp(value) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleString(app.lang, { dateStyle: 'medium', timeStyle: 'short' });
}
export function heading(memory) {
  if (memory.title) return memory.title;
  const line = (memory.content || '').split('\n').find((l) => l.trim()) || memory.id;
  return line.replace(/^[#>*\-\s]+/, '').slice(0, 120);
}
export function owned(project) {
  return !project.includes('/');
}
