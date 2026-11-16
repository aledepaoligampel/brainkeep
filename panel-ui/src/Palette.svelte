<script>
  import { untrack } from 'svelte';
  import { Dialog } from 'bits-ui';
  import { app, t, api, heading, go, memoryHref, listHref } from './lib/state.svelte.js';

  let { open = $bindable(false) } = $props();
  let query = $state('');
  let options = $state([]);
  let active = $state(0);
  let seq = 0;
  let timer = 0;
  let input = $state(null);

  async function search() {
    const mine = ++seq;
    const needle = query.trim().toLowerCase();
    try {
      const found = await api('memories', { q: query.trim(), limit: 6 });
      if (mine !== seq) return;
      const facets = app.facets || { topics: [], projects: [] };
      const match = (v) => !needle || v.toLowerCase().includes(needle);
      options = [
        ...found.items.map((m) => ({ group: t('groups')[0], tag: t('kinds')[m.kind] || m.kind, label: heading(m), meta: m.project,
          target: memoryHref(m.project, m.id, {}, false) })),
        ...facets.topics.filter(match).slice(0, 4).map((x) => ({ group: t('groups')[1], tag: t('topic'), label: x, meta: '',
          target: listHref('', { topic: x }) })),
        ...facets.projects.filter(match).slice(0, 4).map((p) => ({ group: t('groups')[2], tag: t('project'), label: p, meta: '',
          target: listHref(p, {}) })),
      ];
      active = 0;
    } catch { if (mine === seq) options = []; }
  }
  // Reset only when the palette opens; typing must not re-run this.
  $effect(() => { if (open) untrack(() => { query = ''; search(); }); });

  function choose(option) { open = false; go(option.target); }
  function keydown(event) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!options.length) return;
      active = (active + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length;
      document.getElementById('opt-' + active)?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter' && options[active]) {
      event.preventDefault();
      choose(options[active]);
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Portal>
    <Dialog.Overlay class="overlay" />
    <Dialog.Content class="palette" aria-label={t('palette')} onOpenAutoFocus={(e) => { e.preventDefault(); input?.focus(); }}>
      <Dialog.Title class="sr">{t('palette')}</Dialog.Title>
      <div class="p-search">
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5"></path></svg>
        <input type="search" bind:this={input} bind:value={query} placeholder={t('palette')} autocomplete="off" role="combobox" aria-expanded="true"
          aria-controls="palette-list" aria-activedescendant={options.length ? 'opt-' + active : undefined} aria-label={t('palette')}
          oninput={() => { clearTimeout(timer); timer = setTimeout(search, 150); }} onkeydown={keydown}>
        <kbd>Esc</kbd>
      </div>
      <div class="p-list" role="listbox" id="palette-list">
        {#each options as option, n}
          {#if n === 0 || options[n - 1].group !== option.group}<div class="p-group">{option.group}</div>{/if}
          <a class="p-opt" id={'opt-' + n} role="option" aria-selected={n === active} href={option.target}
            onclick={(e) => { e.preventDefault(); choose(option); }} onmousemove={() => (active = n)}>
            <span class="p-tag">{option.tag}</span><span class="p-label">{option.label}</span><span class="muted mono p-meta">{option.meta}</span>
          </a>
        {:else}
          <div class="empty">{t('none')}</div>
        {/each}
      </div>
      <div class="p-foot">{#each t('keys') as k}<span>{k}</span>{/each}</div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  :global(.palette) { position: fixed; z-index: 50; left: 50%; top: 12vh; transform: translateX(-50%); width: min(640px, calc(100vw - 32px));
    max-height: 70vh; display: flex; flex-direction: column; border-radius: 20px; background: var(--surface); border: 1px solid var(--line);
    box-shadow: 0 32px 80px -20px rgba(45, 25, 33, .45); overflow: hidden; }
  .p-search { display: flex; align-items: center; gap: 10px; padding: 0 18px; height: 58px; border-bottom: 1px solid var(--line); color: var(--muted); }
  .p-search input { flex: 1; min-width: 0; border: 0; outline: 0; background: transparent; font-size: 16px; color: var(--ink); }
  svg { width: 17px; height: 17px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; }
  kbd { font: 11px var(--mono); padding: 2px 6px; border: 1px solid var(--line); border-radius: 6px; }
  .p-list { overflow-y: auto; padding: 8px; }
  .p-group { padding: 10px 10px 4px; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); }
  .p-opt { display: flex; align-items: center; gap: 10px; min-height: 42px; padding: 0 10px; border-radius: 10px; color: var(--ink); text-decoration: none; }
  .p-opt[aria-selected="true"] { background: var(--accent-soft); color: var(--ink); }
  .p-tag { flex: 0 0 auto; font-size: 11px; font-weight: 600; color: var(--accent); min-width: 72px; }
  .p-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .p-meta { font-size: 12px; }
  .p-foot { display: flex; gap: 16px; padding: 10px 18px; border-top: 1px solid var(--line); font-size: 12px; color: var(--muted); }
</style>
