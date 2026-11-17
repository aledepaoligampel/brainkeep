<script>
  import { onMount } from 'svelte';
  import Brand from './lib/Brand.svelte';
  import { AGENTS, GITHUB, SITE, REPO } from './lib/agents.js';
  import { app, t, post, boot, setLang } from './lib/state.svelte.js';

  let google = $state(false);
  let token = $state('');
  let busy = $state(false);
  let message = $state('');
  let input;

  onMount(() => {
    fetch('/panel/api/login', { credentials: 'same-origin', cache: 'no-store' })
      .then((r) => (r.ok ? r.json() : null))
      .then((config) => { google = Boolean(config && config.google === true); })
      .catch(() => null);
  });

  async function submit(event) {
    event.preventDefault();
    busy = true;
    message = '';
    try {
      const response = await post('login', { token: token.trim() });
      token = '';
      if (response.ok) { await boot(); return; }
      message = response.status === 429 ? t('throttled') : t('badToken');
    } catch {
      message = t('error');
    }
    busy = false;
    input?.focus();
  }
</script>

<div class="login">
  <section class="hero">
    <Brand size={40} />
    <div class="words">
      <h1>Brainkeep</h1>
      <p>{t('tagline')}</p>
      <nav class="links">
        <a href={SITE} target="_blank" rel="noopener noreferrer">brainkeep.dev</a>
        <a href={REPO} target="_blank" rel="noopener noreferrer"><svg viewBox="0 0 24 24" aria-hidden="true"><path d={GITHUB}></path></svg>GitHub</a>
      </nav>
    </div>
    <ul class="agents">
      {#each AGENTS as agent}
        <li><svg viewBox="0 0 24 24" aria-hidden="true"><path d={agent.d}></path></svg>{agent.name}</li>
      {/each}
    </ul>
  </section>

  <main class="side">
    <div class="box">
      <div class="head">
        <h2>{t('welcome')}</h2>
        <p>{google ? t('loginHelp') : t('keyHelp')}</p>
      </div>
      {#if google}
        <a class="google" href="/oauth/panel/start">
          <svg viewBox="0 0 48 48" aria-hidden="true"><path fill="#EA4335" d="M24 9.5c3.5 0 6.6 1.2 9.1 3.6l6.8-6.8C35.8 2.4 30.3 0 24 0 14.6 0 6.6 5.4 2.7 13.3l7.9 6.1C12.5 13.6 17.8 9.5 24 9.5z"></path><path fill="#4285F4" d="M46.5 24.5c0-1.6-.1-3.1-.4-4.5H24v9h12.7c-.6 3-2.3 5.5-4.8 7.2l7.7 6c4.5-4.2 6.9-10.3 6.9-17.7z"></path><path fill="#FBBC05" d="M10.5 28.6c-.5-1.4-.8-3-.8-4.6s.3-3.2.8-4.6l-7.9-6.1C1 16.6 0 20.2 0 24s1 7.4 2.7 10.7l7.8-6.1z"></path><path fill="#34A853" d="M24 48c6.5 0 11.9-2.1 15.9-5.8l-7.7-6c-2.2 1.5-5 2.3-8.2 2.3-6.2 0-11.5-4.1-13.4-9.8l-7.9 6.1C6.6 42.6 14.6 48 24 48z"></path></svg>
          {t('google')}
        </a>
        <div class="or"><span></span>{t('orKey')}<span></span></div>
      {/if}
      <form onsubmit={submit}>
        <label class="sr" for="token">{t('key')}</label>
        <input id="token" bind:this={input} bind:value={token} type="password" autocomplete="off" spellcheck="false" required placeholder={t('keyPlaceholder')}>
        <button class="btn primary big" type="submit" disabled={busy}>{t('enter')}</button>
      </form>
      {#if message}<p class="error" role="alert">{message}</p>{/if}
      <dl class="kinds">
        {#each t('credentialKinds') as [name, help]}<dt>{name}</dt><dd>{help}</dd>{/each}
      </dl>
      <p class="muted small">{t('noAccess')}</p>
      <div class="seg" role="group" aria-label={t('language')}>
        <button type="button" aria-pressed={app.lang === 'es'} onclick={() => setLang('es')}>ES</button>
        <button type="button" aria-pressed={app.lang === 'en'} onclick={() => setLang('en')}>EN</button>
      </div>
    </div>
  </main>
</div>

<style>
  .login { min-height: 100vh; display: flex; flex-wrap: wrap; }
  .hero { position: relative; overflow: hidden; flex: 1 1 520px; padding: 56px; background: var(--side); display: flex; flex-direction: column;
    justify-content: flex-start; gap: 32px; }
  .words { position: relative; display: flex; flex-direction: column; gap: 20px; max-width: 560px; }
  h1 { margin: 0; font: italic 400 84px/1 var(--serif); letter-spacing: -0.02em; }
  .words p { margin: 0; font: 400 26px/1.3 var(--serif); color: var(--ink-2); }
  .links { display: flex; flex-wrap: wrap; gap: 8px 20px; font-size: 14px; }
  .links a { display: inline-flex; align-items: center; gap: 8px; color: var(--ink-2); text-decoration: none; }
  .links a:hover { color: var(--ink); }
  .links svg { width: 15px; height: 15px; fill: currentColor; }
  .agents { position: relative; margin: auto 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 10px 22px;
    font-size: 13px; color: var(--muted); }
  .agents li { display: flex; align-items: center; gap: 8px; }
  .agents svg { width: 16px; height: 16px; fill: currentColor; opacity: .9; }
  .side { flex: 1 1 440px; padding: 56px 32px; display: flex; align-items: center; justify-content: center; }
  .box { width: 100%; max-width: 380px; display: flex; flex-direction: column; gap: 24px; }
  .head { display: flex; flex-direction: column; gap: 6px; }
  h2 { margin: 0; font: 400 40px/1.05 var(--serif); letter-spacing: -0.015em; }
  .head p { margin: 0; color: var(--ink-2); }
  .google { height: 52px; display: flex; align-items: center; justify-content: center; gap: 12px; border-radius: 999px; border: 1px solid var(--line-strong);
    background: var(--surface); color: var(--ink); font-size: 15px; font-weight: 600; text-decoration: none; box-shadow: var(--shadow); }
  .google:hover { background: var(--tint); color: var(--ink); }
  .google svg { width: 18px; height: 18px; }
  .or { display: flex; align-items: center; gap: 12px; color: var(--muted); font-size: 12px; }
  .or span { flex: 1; height: 1px; background: var(--line); }
  form { display: flex; gap: 8px; }
  input { flex: 1; min-width: 0; height: 46px; padding: 0 16px; border: 1px solid var(--line-strong); border-radius: 999px; background: var(--surface); }
  .small { margin: 0; font-size: 13px; }
  .kinds { margin: 0; display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; font-size: 13px; color: var(--ink-2); }
  .kinds dt { font-weight: 600; color: var(--ink); }
  .kinds dd { margin: 0; }
  .seg { align-self: flex-start; }
  @media (max-width: 640px) {
    .hero { padding: 32px 24px; gap: 28px; flex-basis: 100%; }
    h1 { font-size: 56px; }
    .words p { font-size: 20px; }
  }
</style>
