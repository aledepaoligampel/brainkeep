<script>
  import { onMount } from 'svelte';
  import Brand from './lib/Brand.svelte';
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
    <!-- Outline only: a gradient stroke draws itself, then a glint keeps travelling along the lines. -->
    <svg class="ghost" viewBox="0 0 48 48" aria-hidden="true">
      <defs>
        <linearGradient id="ghost-stroke" x1="0" y1="0" x2="48" y2="48" gradientUnits="userSpaceOnUse">
          <stop offset="0" class="g1"></stop>
          <stop offset=".5" class="g2"></stop>
          <stop offset="1" class="g3"></stop>
          <animateTransform attributeName="gradientTransform" type="rotate" values="0 24 24;360 24 24" dur="14s" repeatCount="indefinite"></animateTransform>
        </linearGradient>
        <filter id="ghost-glow" x="-20%" y="-20%" width="140%" height="140%">
          <feGaussianBlur stdDeviation=".45" result="blur"></feGaussianBlur>
          <feMerge><feMergeNode in="blur"></feMergeNode><feMergeNode in="SourceGraphic"></feMergeNode></feMerge>
        </filter>
      </defs>
      <path class="trace" pathLength="1" stroke="url(#ghost-stroke)" d="M19.125 4a6.1 6.1 0 0 1 3.625 1.188V15.75H19.8a4 4 0 1 0 0 2.5h2.95v23.996A7.17 7.17 0 0 1 18.043 44a7.21 7.21 0 0 1-6.837-4.928l-.375-1.129A7.75 7.75 0 0 1 4 30.25c0-2.026.78-3.87 2.053-5.25H13c.966 0 1.75.784 1.75 1.75v.45a4 4 0 1 0 2.5 0v-.45A4.25 4.25 0 0 0 13 22.5H7.918A7.46 7.46 0 0 1 6 17.5a7.5 7.5 0 0 1 7.003-7.482A6.123 6.123 0 0 1 19.125 4"></path>
      <path class="trace" pathLength="1" stroke="url(#ghost-stroke)" d="M28.875 4a6.123 6.123 0 0 1 6.122 6.018A7.5 7.5 0 0 1 42 17.5a7.48 7.48 0 0 1-2.597 5.67a7.75 7.75 0 0 1-2.234 14.773l-.375 1.13A7.21 7.21 0 0 1 29.957 44c-1.8 0-3.444-.663-4.707-1.754V32.25H29A4.25 4.25 0 0 0 33.25 28v-.201a4 4 0 1 0-2.5 0V28A1.75 1.75 0 0 1 29 29.75h-3.75V5.188A6.1 6.1 0 0 1 28.875 4"></path>
      <path class="trace" pathLength="1" stroke="url(#ghost-stroke)" d="M16 29.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 0 1 0-3"></path>
      <path class="trace" pathLength="1" stroke="url(#ghost-stroke)" d="M32 22.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 0 1 0-3"></path>
      <path class="trace" pathLength="1" stroke="url(#ghost-stroke)" d="M16 15.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 0 1 0-3"></path>
      <path class="glint" pathLength="1" filter="url(#ghost-glow)" style="--i:0" d="M19.125 4a6.1 6.1 0 0 1 3.625 1.188V15.75H19.8a4 4 0 1 0 0 2.5h2.95v23.996A7.17 7.17 0 0 1 18.043 44a7.21 7.21 0 0 1-6.837-4.928l-.375-1.129A7.75 7.75 0 0 1 4 30.25c0-2.026.78-3.87 2.053-5.25H13c.966 0 1.75.784 1.75 1.75v.45a4 4 0 1 0 2.5 0v-.45A4.25 4.25 0 0 0 13 22.5H7.918A7.46 7.46 0 0 1 6 17.5a7.5 7.5 0 0 1 7.003-7.482A6.123 6.123 0 0 1 19.125 4"></path>
      <path class="glint" pathLength="1" filter="url(#ghost-glow)" style="--i:1" d="M28.875 4a6.123 6.123 0 0 1 6.122 6.018A7.5 7.5 0 0 1 42 17.5a7.48 7.48 0 0 1-2.597 5.67a7.75 7.75 0 0 1-2.234 14.773l-.375 1.13A7.21 7.21 0 0 1 29.957 44c-1.8 0-3.444-.663-4.707-1.754V32.25H29A4.25 4.25 0 0 0 33.25 28v-.201a4 4 0 1 0-2.5 0V28A1.75 1.75 0 0 1 29 29.75h-3.75V5.188A6.1 6.1 0 0 1 28.875 4"></path>
      <path class="glint" pathLength="1" filter="url(#ghost-glow)" style="--i:2" d="M16 29.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 0 1 0-3"></path>
      <path class="glint" pathLength="1" filter="url(#ghost-glow)" style="--i:3" d="M32 22.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 0 1 0-3"></path>
      <path class="glint" pathLength="1" filter="url(#ghost-glow)" style="--i:4" d="M16 15.5a1.5 1.5 0 1 1 0 3a1.5 1.5 0 0 1 0-3"></path>
    </svg>
    <Brand size={40} />
    <div class="words">
      <h1>Brainkeep</h1>
      <p>{t('tagline')}</p>
    </div>
    <p class="agents">Claude · Codex · Antigravity · claude.ai</p>
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
    justify-content: space-between; gap: 48px; }
  .ghost { position: absolute; right: -150px; bottom: -170px; width: 600px; height: 600px; }
  .ghost path { fill: none; stroke-linecap: round; stroke-linejoin: round; }
  .ghost .g1 { stop-color: var(--accent); }
  .ghost .g2 { stop-color: var(--brand-deep); }
  .ghost .g3 { stop-color: var(--accent-ink); }
  .ghost .trace { stroke-width: .24; stroke-dasharray: 1.05; stroke-dashoffset: 1.05; animation: ghost-draw 3.2s cubic-bezier(.65, 0, .35, 1) .2s forwards; }
  .ghost .glint { stroke: var(--accent); stroke-width: .42; stroke-dasharray: .08 .92; stroke-dashoffset: 1; opacity: 0;
    animation: ghost-glint 7s linear calc(3.2s + var(--i) * .9s) infinite; }
  @keyframes ghost-draw { to { stroke-dashoffset: 0; } }
  @keyframes ghost-glint { 0% { stroke-dashoffset: 1; opacity: 0; } 8%, 85% { opacity: 1; } 100% { stroke-dashoffset: 0; opacity: 0; } }
  @media (prefers-reduced-motion: reduce) {
    .ghost .trace { animation: none; stroke-dashoffset: 0; }
    .ghost .glint { display: none; }
  }
  .words { position: relative; display: flex; flex-direction: column; gap: 20px; max-width: 560px; }
  h1 { margin: 0; font: italic 400 84px/1 var(--serif); letter-spacing: -0.02em; }
  .words p { margin: 0; font: 400 26px/1.3 var(--serif); color: var(--ink-2); }
  .agents { position: relative; margin: 0; font-size: 13px; color: var(--muted); }
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
    .ghost { width: 360px; height: 360px; right: -110px; bottom: -120px; }
  }
</style>
