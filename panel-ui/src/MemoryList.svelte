<script>
  import { untrack } from 'svelte';
  import AgentIcon from './lib/AgentIcon.svelte';
  import FilterMenu from './lib/FilterMenu.svelte';
  import DateFilter from './lib/DateFilter.svelte';
  import Share from './Share.svelte';
  import { KINDS } from './lib/i18n.js';
  import { app, t, api, when, heading, go, listHref, memoryHref, owned, FILTERS } from './lib/state.svelte.js';

  const route = $derived(app.route);
  const project = $derived(route.project && route.scoped ? route.project : '');
  const filters = $derived(route.filters);
  const info = $derived((app.projects || []).find((p) => p.project === project));

  let items = $state([]);
  let total = $state(0);
  let more = $state(false);
  let snapshot = null;
  let busy = $state(false);
  let failed = $state(false);
  let notice = $state(false);
  let fresh = $state(new Set());
  let facets = $state(null);
  let q = $state('');
  let seq = 0;
  let timer = 0;

  // A newer request supersedes older ones; a stale response is discarded.
  async function load(reset, mark = false) {
    const mine = ++seq;
    const offset = reset ? 0 : items.length;
    busy = true;
    failed = false;
    try {
      // A live reload keeps the pages already shown (the API returns at most 50).
      const limit = mark ? Math.min(50, Math.max(20, items.length)) : 20;
      const page = await api('memories', { ...filters, project, limit, offset: offset || undefined, snapshot: offset ? snapshot : undefined });
      if (mine !== seq) return;
      if (mark) {
        const known = new Set(items.map((m) => m.id + ':' + m.revision));
        fresh = new Set(page.items.filter((m) => !known.has(m.id + ':' + m.revision)).map((m) => m.id));
      } else if (reset) fresh = new Set();
      items = reset ? page.items : [...items, ...page.items];
      total = page.total;
      more = page.more;
      snapshot = page.snapshot;
    } catch (error) {
      if (mine !== seq) return;
      if (error.status === 409) { notice = true; load(true); return; }
      if (error.status !== 401) failed = true;
    } finally {
      if (mine === seq) busy = false;
    }
  }

  $effect(() => {
    project; JSON.stringify(filters);
    untrack(() => { q = filters.q; notice = false; load(true); });
  });
  $effect(() => {
    project; filters.archived;
    untrack(async () => {
      const wanted = project;
      try { const found = await api('facets', { project: wanted, archived: filters.archived }); if (wanted === project) facets = found; } catch { if (wanted === project) facets = null; }
    });
  });
  // Live updates: reload the open listing and mark what is new.
  $effect(() => {
    if (app.changed) untrack(() => load(true, true));
  });

  const target = (next) => (route.id ? memoryHref(route.project, route.id, next) : listHref(project, next));
  function setFilter(name, value) { go(target({ ...filters, [name]: value })); }
  function onSearch() {
    clearTimeout(timer);
    timer = setTimeout(() => setFilter('q', q.trim()), 250);
  }
  const view = $derived(filters.archived ? { archived: '1' } : {});
  function clearAll() { go(route.id ? memoryHref(route.project, route.id, view) : listHref(project, view)); }
  const active = $derived(FILTERS.filter((n) => n !== 'q' && n !== 'kind' && n !== 'archived' && filters[n]).length);
  // The filter pills fold behind a button; the badge counts filters in use.
  const used = $derived(['principal', 'agent', 'topic', 'state'].filter((n) => filters[n]).length + (filters.from || filters.to ? 1 : 0));
  let showFilters = $state(false);
</script>

<section class="list" aria-labelledby="list-title">
  <header class="head">
    <div class="title">
      <h1 id="list-title" class="serif">{project ? project.split('/').slice(-1)[0] : filters.archived ? t('archived') : t('memories')}</h1>
      <span class="muted" title={info ? t('lastSave') + ' ' + when(info.last_saved_at) : undefined}>{t(total === 1 ? 'count1' : 'count', total)}{#if project.includes('/')} · <span class="mono">{project.split('/')[0]}</span>{/if}</span>
    </div>
    {#if project && owned(project) && app.session?.user && !filters.archived}<Share {project} />{/if}
  </header>

  <div class="searchrow">
  <div class="searchbox" role="search">
    <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5"></path></svg>
    <label class="sr" for="q">{t('search')}</label>
    <input id="q" type="search" bind:value={q} oninput={onSearch} placeholder={t('search') + '…'} autocomplete="off">
  </div>
  <button type="button" class="filter-toggle" aria-expanded={showFilters} aria-controls="pills" aria-label={t('filters') + (used ? ` (${used})` : '')}
    onclick={() => (showFilters = !showFilters)}>
    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M7 12h10M10 18h4"></path></svg>
    {#if used}<span class="badge" aria-hidden="true">{used}</span>{/if}
  </button>
  </div>

  <div class="chips" role="group" aria-label={t('kind')}>
    <button type="button" class="chip" aria-pressed={!filters.kind} onclick={() => setFilter('kind', '')}>{t('all')}</button>
    {#each KINDS as kind}
      <button type="button" class="chip" aria-pressed={filters.kind === kind} onclick={() => setFilter('kind', filters.kind === kind ? '' : kind)}>
        <span class="dot {kind}" aria-hidden="true"></span>{t('plural')[kind]}{#if info && info.kinds[kind]}<span class="n">{info.kinds[kind]}</span>{/if}
      </button>
    {/each}
  </div>

  <div class="pills" class:open={showFilters} id="pills" role="group" aria-label={t('filters')}>
    <FilterMenu label={t('principal')} value={filters.principal} options={(facets?.principals || []).map((v) => ({ value: v, label: v }))}
      onchange={(v) => setFilter('principal', v)} />
    <FilterMenu label={t('agent')} value={filters.agent} agents options={(facets?.agents || []).map((v) => ({ value: v, label: v }))}
      onchange={(v) => setFilter('agent', v)} />
    <FilterMenu label={t('topic')} value={filters.topic} options={(facets?.topics || []).map((v) => ({ value: v, label: v }))}
      onchange={(v) => setFilter('topic', v)} />
    <FilterMenu label={t('state')} value={filters.state}
      options={['confirmed', 'proposed'].map((v) => ({ value: v, label: t('states')[v] }))} onchange={(v) => setFilter('state', v)} />
    <DateFilter from={filters.from} to={filters.to} onchange={(from, to) => go(target({ ...filters, from, to }))} />
    {#if active}<button type="button" class="clear" onclick={clearAll}>{t('clear')}</button>{/if}
  </div>

  {#if notice}<div class="error" role="status">{t('changed')}</div>{/if}
  {#if failed}<div class="error" role="alert">{t('error')}</div>{/if}

  <ul class="rows">
    {#each items as m (m.project + '/' + m.id)}
      {@const selected = route.id === m.id && route.project === m.project}
      <li>
        <a class="row" class:selected href={memoryHref(m.project, m.id, filters, Boolean(project))} aria-current={selected ? 'true' : undefined}>
          <span class="line">
            <span class="kind {m.kind}">{t('kinds')[m.kind] || m.kind}</span>
            {#if m.topic}<span class="mono topic">{m.topic}</span>{/if}
            <span class="when">{when(m.saved_at)}</span>
          </span>
          <span class="name">{heading(m)}{#if fresh.has(m.id)}<span class="new">{t('newBadge')}</span>{/if}</span>
          <span class="meta">
            <AgentIcon agent={m.agent} />
            {#if !project}<span class="mono">{m.project}</span> ·{/if}
            <span>rev {m.revision}</span> · <span class="state {m.state}">{t('states')[m.state] || m.state}</span>
          </span>
        </a>
      </li>
    {/each}
  </ul>
  {#if !busy && !failed && !items.length}<div class="empty">{t('none')}</div>{/if}
  <div class="foot">
    {#if total}<span class="muted">{t('range', 1, items.length, total)}</span>{/if}
    {#if more}<button type="button" class="btn" disabled={busy} onclick={() => load(false)}>{t('more')}</button>{/if}
  </div>
</section>

<style>
  .list { min-width: 0; padding: 32px 24px 32px 32px; display: flex; flex-direction: column; gap: 16px; }
  /* Title on the left, project actions (Share) on the right of the same line. */
  .head { display: flex; align-items: center; justify-content: space-between; gap: 8px 16px; flex-wrap: wrap; }
  .title { display: flex; align-items: baseline; gap: 12px; min-width: 0; flex: 1 1 auto; }
  .title > .muted { flex: 0 0 auto; white-space: nowrap; }
  h1 { margin: 0; font-size: 38px; line-height: 1.05; letter-spacing: -0.015em; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .searchbox { display: flex; align-items: center; gap: 8px; height: 40px; padding: 0 14px; border-radius: 12px; background: var(--surface);
    border: 1px solid var(--line); color: var(--muted); }
  .searchbox input { flex: 1; min-width: 0; border: 0; outline: 0; background: transparent; color: var(--ink); }
  svg { width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; }
  .chips { display: flex; gap: 6px; overflow-x: auto; scrollbar-width: none; margin: 0 -4px; padding: 2px 4px; }
  .chips::-webkit-scrollbar { display: none; }
  .chips { mask-image: linear-gradient(to right, #000 calc(100% - 28px), transparent); padding-right: 28px; }
  .chip { flex: 0 0 auto; display: inline-flex; align-items: center; gap: 7px; height: 32px; padding: 0 13px; border-radius: 999px; border: 1px solid var(--line);
    background: var(--surface); color: var(--ink-2); font-size: 13px; font-weight: 500; cursor: pointer; white-space: nowrap; }
  .chip:hover { border-color: var(--line-strong); color: var(--ink); }
  .chip[aria-pressed="true"] { background: var(--ink); border-color: var(--ink); color: var(--bg); font-weight: 600; }
  .chip .n { font-size: 11px; opacity: .65; font-variant-numeric: tabular-nums; }
  .dot { width: 7px; height: 7px; border-radius: 50%; }
  .dot.decision { background: var(--k-decision); } .dot.progress { background: var(--k-progress); } .dot.discovery { background: var(--k-discovery); }
  .dot.preference { background: var(--k-preference); } .dot.note { background: var(--k-note); }
  .chip[aria-pressed="true"] .dot { box-shadow: 0 0 0 2px var(--bg); }
  .pills { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding-top: 2px; }
  .searchrow { display: flex; gap: 8px; }
  .searchrow .searchbox { flex: 1; min-width: 0; }
  .filter-toggle { display: inline-flex; position: relative; flex: 0 0 auto; width: 40px; height: 40px; align-items: center; justify-content: center;
    border-radius: 12px; border: 1px solid var(--line); background: var(--surface); color: var(--ink-2); cursor: pointer; }
  .filter-toggle[aria-expanded="true"] { border-color: var(--accent); color: var(--ink); }
  .filter-toggle svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; }
  .filter-toggle .badge { position: absolute; top: -5px; right: -5px; min-width: 18px; height: 18px; padding: 0 5px; border-radius: 999px;
    background: var(--accent); color: var(--surface); font-size: 11px; font-weight: 600; display: inline-flex; align-items: center; justify-content: center; }
  .pills:not(.open) { display: none; }
  .clear { height: 30px; padding: 0 8px; border: 0; background: none; color: var(--accent); font-size: 13px; font-weight: 500; cursor: pointer; border-radius: 8px; }
  .clear:hover { background: var(--accent-soft); }
  .rows { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .row { display: flex; flex-direction: column; gap: 6px; padding: 14px 16px; border-radius: 14px; border: 1px solid transparent;
    color: var(--ink); text-decoration: none; }
  .row:hover { background: var(--surface); color: var(--ink); }
  .row.selected { background: var(--surface); border-color: var(--accent-line); box-shadow: 0 1px 2px rgba(29, 21, 25, .04), 0 6px 16px -8px rgba(120, 40, 70, .18); }
  .line { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .topic { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { margin-left: auto; font-size: 12px; color: var(--muted); white-space: nowrap; }
  .name { font-size: 15px; font-weight: 550; letter-spacing: -0.005em; overflow-wrap: anywhere; }
  .new { margin-left: 8px; font-size: 11px; font-weight: 600; color: var(--accent); }
  .meta { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 12px; color: var(--muted); }
  .foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  @media (max-width: 720px) { .list { padding: 20px 16px; } h1 { font-size: 30px; } }
</style>
