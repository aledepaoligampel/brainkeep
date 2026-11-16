// Live updates over Server-Sent Events. The server only names visible projects that
// changed; the open views reload themselves. Nothing is polled while idle.
import { app, api, refreshShared } from './state.svelte.js';

let source = null;
let retry = 0;
let timer = 0;
let pending = 0;

const MAX_RETRIES = 8;

function changed() {
  // Coalesce bursts of writes into one reload; jitter spreads tabs over time.
  clearTimeout(pending);
  pending = setTimeout(() => { app.changed += 1; refreshShared(false); }, 300 + Math.random() * 600);
}

export function connect() {
  if (source || typeof EventSource === 'undefined') return;
  clearTimeout(timer);
  source = new EventSource('/panel/api/events', { withCredentials: true });
  source.addEventListener('open', () => {
    // A reconnection may have missed events: refresh once.
    if (retry > 0) changed();
    retry = 0;
    app.connection = 'live';
  });
  source.addEventListener('change', changed);
  source.addEventListener('error', () => {
    if (!source) return;
    source.close();
    source = null;
    app.connection = retry < 2 ? 'reconnecting' : 'offline';
    retry += 1;
    // Give up after a while (e.g. too many tabs); focusing the window tries again.
    if (retry > MAX_RETRIES) return;
    // The session may have ended (logout elsewhere, user disabled): ask once.
    api('session').then(() => {
      timer = setTimeout(connect, Math.min(30000, 1000 * 2 ** Math.min(retry, 5)));
    }).catch((error) => {
      if (error.status !== 401) timer = setTimeout(connect, Math.min(30000, 1000 * 2 ** Math.min(retry, 5)));
    });
  });
}

window.addEventListener('focus', () => {
  if (app.session && !source && retry > MAX_RETRIES) { retry = 1; connect(); }
});

export function disconnect() {
  clearTimeout(timer);
  if (source) source.close();
  source = null;
  retry = 0;
  app.connection = 'idle';
}
