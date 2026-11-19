<script>
  // How to connect each agent. With Google sign-in, OAuth first: no token travels by
  // chat, and a personal token remains for machines without a browser (issuing one
  // replaces the previous token). Without it, the token is the way in (lib/connect.js).
  import { app, t, send } from './lib/state.svelte.js';
  import { AGENT_MARKS, REPO } from './lib/agents.js';
  import { connectView } from './lib/connect.js';
  import PageHeader from './lib/PageHeader.svelte';

  const url = location.origin + '/mcp';
  const clients = ['claude-code', 'codex', 'antigravity', 'claude-ai', 'chatgpt'];
  let client = $state('claude-code');
  let token = $state('');
  let busy = $state(false);
  let error = $state('');
  let copied = $state('');

  const view = $derived(connectView({ client, google: app.google, session: app.session, url, token }));
  const steps = $derived(t(view.stepsKey)[client]);
  const guide = REPO + '/blob/main/docs/railway.md';

  async function copy(text, key) {
    try { await navigator.clipboard.writeText(text); copied = key; setTimeout(() => { if (copied === key) copied = ''; }, 2000); }
    catch { copied = ''; }
  }
  async function issue() {
    busy = true;
    error = '';
    const r = await send('my_token', {});
    busy = false;
    if (r.ok) token = r.body.token;
    else error = { 'token was just issued; try again in a minute': t('tokenTooSoon'),
      'sign in with Google to issue a new token': t('tokenSignedInWithToken') }[r.body?.error] || t('error');
  }
</script>

<section class="page connect" aria-labelledby="connect-title">
  <PageHeader id="connect-title" title={t('connect')} description={t(view.intro)} />

  <div class="field">
    <span class="label">{t('serverUrl')}</span>
    <div class="secret">
      <code>{url}</code>
      <button type="button" class="btn" onclick={() => copy(url, 'url')}>{copied === 'url' ? t('copiedShort') : t('copyShort')}</button>
    </div>
  </div>

  {#if app.google !== null}
  <div class="tabs" role="tablist" aria-label={t('connect')}>
    {#each clients as id}
      <button type="button" role="tab" aria-selected={client === id} class:on={client === id} onclick={() => (client = id)}>
        {#if AGENT_MARKS[id]}<svg viewBox="0 0 24 24" aria-hidden="true"><path d={AGENT_MARKS[id]}></path></svg>{/if}
        {t('clientNames')[id]}
      </button>
    {/each}
  </div>

  <div class="panel" role="tabpanel">
    {#if view.primary}
      <ol class="steps">
        {#each view.primary as block, i}
          <li>
            <p>{steps[i]}</p>
            <div class="code">
              <pre>{block}</pre>
              <button type="button" class="btn" onclick={() => copy(block, 'o' + i)}>{copied === 'o' + i ? t('copiedShort') : t('copyShort')}</button>
            </div>
          </li>
        {/each}
        <li><p>{steps[view.primary.length]}</p></li>
      </ol>
    {/if}
    {#if view.tokenSection}
      {#if t('connectNotes')[client]}<p class="muted small">{t('connectNotes')[client]}</p>{/if}
    {:else}
      <p class="small">{t(view.note)}{#if view.note === 'tokenAdminNoGoogle'} <a href={guide} target="_blank" rel="noopener noreferrer">docs/railway.md</a>{/if}</p>
    {/if}
  </div>

  {#if view.tokenSection}
    <details class="token">
      <summary>{t('tokenSectionTitle')}</summary>
      <p class="muted small">{t('tokenSectionHelp')}</p>
      {#if view.note}
        <p class="small">{t(view.note)}</p>
      {:else}
        {#if token}
          <p class="warn small">{t('tokenOnceSelf')}</p>
        {:else}
          <button type="button" class="btn" disabled={busy} onclick={issue}>{busy ? t('working') : t('generateToken')}</button>
          {#if error}<p class="error small" role="alert">{error}</p>{/if}
        {/if}
        {#each view.token as block, i}
          <div class="code">
            <pre>{block}</pre>
            <button type="button" class="btn" disabled={!token} onclick={() => copy(block, 't' + i)}>{copied === 't' + i ? t('copiedShort') : t('copyShort')}</button>
          </div>
        {/each}
      {/if}
    </details>
  {/if}
  {/if}
</section>

<style>
  .connect > :not(:first-child) { max-width: 880px; }
  .field { display: flex; flex-direction: column; gap: 8px; }
  .label { font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); }
  .tabs { display: flex; flex-wrap: wrap; gap: 6px; }
  .tabs button { display: inline-flex; align-items: center; gap: 8px; height: 36px; padding: 0 14px; border-radius: 999px;
    border: 1px solid var(--line); background: var(--surface); color: var(--ink-2); font-size: 13px; cursor: pointer; }
  .tabs button.on { background: var(--ink); border-color: var(--ink); color: var(--bg); }
  .tabs svg { width: 15px; height: 15px; fill: currentColor; }
  .panel { display: flex; flex-direction: column; gap: 12px; padding: 20px; border: 1px solid var(--line); border-radius: 16px; background: var(--surface); }
  .steps { margin: 0; padding-left: 20px; display: flex; flex-direction: column; gap: 14px; }
  .steps p { margin: 0 0 8px; }
  .code { display: flex; align-items: center; gap: 8px; padding: 10px 10px 10px 14px; border-radius: 12px; background: var(--bg); border: 1px solid var(--line); }
  .code pre { flex: 1; min-width: 0; margin: 0; font: 12.5px/1.55 var(--mono); white-space: pre-wrap; overflow-wrap: anywhere; color: var(--ink); }
  .token { display: flex; flex-direction: column; gap: 12px; }
  .token summary { cursor: pointer; font-weight: 600; }
  .token[open] { display: flex; }
  .token > * + * { margin-top: 12px; }
  .small { margin: 0; font-size: 13px; }
  .warn { color: var(--warn); }
  .error { color: var(--danger); }
</style>
