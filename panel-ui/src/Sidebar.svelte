<script>
  import { DropdownMenu } from 'bits-ui';
  import Brand from './lib/Brand.svelte';
  import AgentIcon from './lib/AgentIcon.svelte';
  import { app, t, listHref, owned, setLang, setTheme, logout, when, refreshShared } from './lib/state.svelte.js';

  let { onsearch } = $props();
  const mine = $derived((app.projects || []).filter((p) => owned(p.project)));
  const shared = $derived((app.projects || []).filter((p) => !owned(p.project)));
  const user = $derived(app.session?.user || '');
  const display = $derived(app.session?.name || user || t('admin'));
  const picture = $derived(app.session?.picture || '');
  let broken = $state('');
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
  const connection = $derived({
    live: [t('live'), 'pulse'], reconnecting: [t('reconnecting'), 'warn'], offline: [t('offline'), 'off'], idle: [t('live'), ''],
  }[app.connection]);

  // Fade the project list's edges only where more items hide past them.
  let lists = $state();
  let fadeStart = $state(false);
  let fadeEnd = $state(false);
  function edges() {
    if (!lists) return;
    const { scrollTop, scrollLeft, scrollHeight, scrollWidth, clientHeight, clientWidth } = lists;
    fadeStart = scrollTop > 1 || scrollLeft > 1;
    fadeEnd = scrollTop + clientHeight < scrollHeight - 1 || scrollLeft + clientWidth < scrollWidth - 1;
  }
  $effect(() => {
    void mine.length; void shared.length; void app.session?.admin;
    if (!lists) return;
    const observer = new ResizeObserver(edges);
    observer.observe(lists);
    for (const child of lists.children) observer.observe(child);
    edges();
    return () => observer.disconnect();
  });
</script>

<nav class="side" aria-label={t('projects')}>
  <a class="brand" href="#/"><Brand size={34} /><span>Brainkeep</span></a>

  <button type="button" class="search" onclick={onsearch}>
    <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5"></path></svg>
    <span>{t('search')}</span>
    <kbd>{isMac ? '⌘K' : 'Ctrl K'}</kbd>
  </button>

  <div class="lists" class:fade-start={fadeStart} class:fade-end={fadeEnd} bind:this={lists} onscroll={edges}>
  <div class="group">
    <a class="nav" class:on={!app.route.scoped && !app.route.users && !app.route.connect && !app.route.filters.archived} href={listHref('', {})}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M4 12h16M4 18h10"></path></svg>{t('memories')}
      {#if app.status}<span class="count">{app.status.memories}</span>{/if}
    </a>
    {#if app.status?.archived || app.route.filters.archived}
      <a class="nav" class:on={!app.route.scoped && !app.route.users && !app.route.connect && app.route.filters.archived} href={listHref('', { archived: '1' })}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="4" width="18" height="5" rx="1.5"></rect><path d="M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9M10 13h4"></path></svg>{t('archived')}
        <span class="count">{app.status?.archived || 0}</span>
      </a>
    {/if}
  </div>

  {#if mine.length}
    <div class="group">
      <h2 class="label">{t('myProjects')}</h2>
      {#each mine as p (p.project)}
        <a class="nav proj" class:on={app.route.project === p.project && app.route.scoped} href={listHref(p.project, {})}>
          <span class="dot" aria-hidden="true"></span><span class="mono name">{p.project}</span><span class="count">{p.total}</span>
        </a>
      {/each}
    </div>
  {/if}

  {#if shared.length}
    <div class="group">
      <h2 class="label">{t('shared')}</h2>
      {#each shared as p (p.project)}
        <a class="nav proj" class:on={app.route.project === p.project && app.route.scoped} href={listHref(p.project, {})}>
          <span class="initial" aria-hidden="true">{p.project[0].toUpperCase()}</span><span class="mono name">{p.project}</span><span class="count">{p.total}</span>
        </a>
      {/each}
    </div>
  {/if}

  <div class="group">
    <a class="nav" class:on={app.route.connect} href="#/connect" aria-current={app.route.connect ? 'page' : undefined}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 7V3M15 7V3M7 7h10v4a5 5 0 0 1-10 0V7ZM12 16v5"></path></svg>{t('connect')}
    </a>
  </div>

  {#if app.session?.admin}
    <div class="group">
      <h2 class="label">{t('administration')}</h2>
      <a class="nav" class:on={app.route.users} href="#/users" aria-current={app.route.users ? 'page' : undefined}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="9" cy="8" r="3.5"></circle><path d="M2.5 20a6.5 6.5 0 0 1 13 0"></path><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18 14a6 6 0 0 1 3.5 6"></path></svg>{t('users')}
      </a>
    </div>
  {/if}

  </div>

  {#if app.activity?.length}
    <section class="activity" aria-label={t(app.activity.length === 1 ? 'activeAgents1' : 'activeAgents', app.activity.length)}>
      <span class="faces" aria-hidden="true">
        {#each app.activity.slice(0, 5) as a (a.agent)}<span class="face" title={`${a.agent} · ${a.revisions}`}><AgentIcon agent={a.agent} size={22} label={false} /></span>{/each}
      </span>
      <span class="activity-text">{t(app.activity.length === 1 ? 'activeAgents1' : 'activeAgents', app.activity.length)}</span>
      <span class="sr-only">{app.activity.map((a) => `${a.agent}: ${a.revisions}`).join(', ')}</span>
    </section>
  {/if}

  <div class="me">
      <DropdownMenu.Root onOpenChange={(open) => { if (open) refreshShared(); }}>
        <DropdownMenu.Trigger class="me-row" aria-label={t('account') + ': ' + display + ', ' + connection[0]}>
      <span class="avatar" title={connection[0]}>
        {#if picture && broken !== picture}
          <img src={picture} alt="" referrerpolicy="no-referrer" onerror={() => (broken = picture)} />
        {:else}
          {[...(app.session?.name || user || 'a')][0]}
        {/if}
        <span class="live-dot {connection[1]}" aria-hidden="true"></span>
      </span>
      <span class="who">
        <span class="handle" title={user || undefined}>{display}</span>
        {#if app.session?.email}<span class="tag" title={app.session.email}>{app.session.email}</span>
        {:else if !user}<span class="tag">{t('adminKey')}</span>{/if}
      </span>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content class="menu" side="top" align="end" sideOffset={8} collisionPadding={12}>
            {#if app.session}<div class="menu-row"><span class="muted">{t('expires', when(app.session.expires_at))}</span></div>{/if}
            {#if app.status}
              <dl class="kv">
                <dt>{t('lastSave')}</dt><dd>{app.status.last_save ? when(app.status.last_save) : '—'}</dd>
                <dt>{t('memories')}</dt><dd>{app.status.memories}</dd>
                <dt>{t('projects')}</dt><dd>{app.status.projects}</dd>
                <dt>{t('version')}</dt><dd class="mono">brainkeep {app.status.version} · v{app.status.schema_version}</dd>
              </dl>
            {:else if app.status === false}
              <p class="error small">{t('offline')}</p>
            {/if}
            <div class="menu-sep"></div>
            <div class="menu-row"><span>{t('language')}</span>
              <span class="seg" role="group" aria-label={t('language')}>
                <button type="button" aria-pressed={app.lang === 'es'} onclick={() => setLang('es')}>ES</button>
                <button type="button" aria-pressed={app.lang === 'en'} onclick={() => setLang('en')}>EN</button>
              </span></div>
            <div class="menu-row"><span>{t('theme')}</span>
              <span class="seg" role="group" aria-label={t('theme')}>
                {#each ['system', 'light', 'dark'] as value, n}
                  <button type="button" aria-pressed={app.theme === value} onclick={() => setTheme(value)}>{t('themes')[n]}</button>
                {/each}
              </span></div>
            <div class="menu-sep"></div>
            <DropdownMenu.Item class="menu-item danger" onSelect={logout}>
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M14 8V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h7a2 2 0 0 0 2-2v-2M9 12h12l-3-3m0 6 3-3"></path></svg>
              {t('logout')}
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
  </div>
</nav>

<style>
  .side { position: relative; width: 100%; height: 100%; padding: 24px 16px 16px; display: flex; flex-direction: column; gap: 24px; background: var(--side);
    border-right: 1px solid var(--side-line); overflow-y: auto; }
  svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; flex: 0 0 auto; }
  .brand { flex: 0 0 auto; display: flex; align-items: center; gap: 10px; padding: 0 6px; color: var(--ink); text-decoration: none; }
  .brand span { font: italic 400 24px/1 var(--serif); letter-spacing: -0.01em; }
  .search { flex: 0 0 auto; display: flex; align-items: center; gap: 8px; height: 40px; padding: 0 12px; border-radius: 12px; border: 1px solid var(--side-line);
    background: var(--surface); color: var(--muted); cursor: pointer; box-shadow: var(--shadow-sm); text-align: left; }
  .search span { flex: 1; }
  kbd { font: 11px var(--mono); color: var(--muted); }
  /* Projects scroll on their own; padding keeps focus rings inside the clip. */
  .lists { --fade-start: 0px; --fade-end: 0px; display: flex; flex-direction: column; gap: 24px; flex: 1 1 auto; min-height: 0;
    overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; margin: -4px -6px; padding: 4px 6px;
    mask-image: linear-gradient(to bottom, transparent, #000 var(--fade-start), #000 calc(100% - var(--fade-end)), transparent); }
  .lists.fade-start { --fade-start: 32px; }
  .lists.fade-end { --fade-end: 32px; }
  .group { display: flex; flex-direction: column; gap: 2px; }
  .group .label { padding: 0 10px 8px; }
  .nav { display: flex; align-items: center; gap: 10px; min-height: 36px; padding: 0 10px; border-radius: 10px; color: var(--ink-2); text-decoration: none; }
  .nav:hover { background: var(--hover); color: var(--ink); }
  .nav.on { background: var(--raised); color: var(--ink); font-weight: 600; box-shadow: var(--shadow-sm); }
  .nav.on svg { stroke: var(--accent); }
  .name { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count { margin-left: auto; font-size: 12px; color: var(--muted); font-weight: 400; }
  .dot { width: 6px; height: 6px; border-radius: 50%; background: var(--brand-deep); flex: 0 0 auto; }
  .nav.on .dot { background: var(--accent); }
  .initial { width: 18px; height: 18px; border-radius: 50%; background: #E8D7F0; color: #5B2A73; font-size: 10px; font-weight: 600;
    display: inline-flex; align-items: center; justify-content: center; flex: 0 0 auto; }
  .activity { margin-top: auto; display: grid; grid-template-columns: auto 1fr; align-items: center; gap: 6px 10px; padding: 0 10px; font-size: 12px; color: var(--muted); }
  .faces { display: inline-flex; }
  .face { display: inline-flex; border-radius: 50%; background: var(--raised); box-shadow: 0 0 0 2px var(--side), 0 0 0 3px var(--side-line); }
  .face + .face { margin-left: -6px; }
  .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .activity + .me { margin-top: 0; }
  .me { margin-top: auto; border-radius: 18px; background: var(--surface); border: 1px solid var(--side-line); box-shadow: var(--shadow); }
  .me :global(.me-row) { display: flex; align-items: center; gap: 10px; width: 100%; padding: 10px; border: 0; border-radius: 18px; background: transparent;
    color: inherit; font: inherit; text-align: left; cursor: pointer; }
  .me :global(.me-row:hover), .me :global(.me-row[data-state="open"]) { background: var(--hover); }
  .me :global(.me-row:focus-visible) { outline: 2px solid var(--accent); outline-offset: 2px; }
  .avatar { position: relative; width: 36px; height: 36px; flex: 0 0 auto; border-radius: 50%; background: var(--brand);
    box-shadow: 0 0 0 2px var(--surface), 0 0 0 3px var(--brand-deep); display: inline-flex; align-items: center; justify-content: center;
    font: 600 16px var(--sans); color: #5E1F37; text-transform: uppercase; }
  .avatar img { width: 100%; height: 100%; border-radius: 50%; object-fit: cover; }
  .avatar .live-dot { position: absolute; right: -1px; bottom: -1px; width: 11px; height: 11px; border: 2px solid var(--surface); }
  .who { flex: 1; display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .handle { font: 600 14px/1.2 var(--sans); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tag { font-size: 11px; line-height: 1; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  :global(.menu) .kv { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; margin: 4px 10px 6px; font-size: 12px; }
  :global(.menu) .kv dt { color: var(--muted); }
  :global(.menu) .kv dd { margin: 0; text-align: right; }
  .small { margin: 4px 6px; font-size: 12px; }
  /* Phone: a compact top bar; projects scroll sideways. */
  @media (max-width: 800px) {
    .side { flex-direction: row; flex-wrap: wrap; align-items: center; gap: 10px 8px; padding: 12px 16px; border-right: 0;
      border-bottom: 1px solid var(--side-line); overflow: visible; height: auto; align-content: flex-start; }
    .brand { flex: 1; padding: 0; }
    .brand span { font-size: 22px; }
    .search { order: 2; width: 44px; height: 44px; padding: 0; justify-content: center; }
    .search span, .search kbd { display: none; }
    .me { order: 3; margin: 0 0 0 8px; border: 0; box-shadow: none; background: transparent; }
    /* The phone header has room for brand, search and account only. */
    .activity { display: none; }
    .me :global(.me-row) { padding: 0; border-radius: 50%; }
    .me :global(.me-row:hover) { background: transparent; }
    .who { display: none; }
    .avatar { width: 36px; height: 36px; font-size: 17px; }
    .lists { order: 4; flex: 1 1 100%; flex-direction: row; gap: 6px; overflow-x: auto; overflow-y: hidden; scrollbar-width: none;
      min-height: auto; margin: -2px -16px; padding: 2px 16px;
      mask-image: linear-gradient(to right, transparent, #000 var(--fade-start), #000 calc(100% - var(--fade-end)), transparent); }
    .group { flex: 0 0 auto; flex-direction: row; gap: 6px; }
    .group .label { display: none; }
    .nav { flex: 0 0 auto; min-height: 36px; border: 1px solid var(--side-line); background: var(--surface); }
    .count { margin-left: 4px; }
  }
</style>
