<script>
  import { untrack } from 'svelte';
  import { AlertDialog } from 'bits-ui';
  import AgentIcon from './lib/AgentIcon.svelte';
  import MemoryEditor from './MemoryEditor.svelte';
  import MdNode from './lib/MdNode.svelte';
  import { parseMarkdown, lineDiff, author } from './lib/markdown.js';
  import { app, t, api, post, go, stamp, when, heading, listHref, refreshShared } from './lib/state.svelte.js';

  let { project, id } = $props();

  let data = $state(null);
  let revisions = $state([]);
  let nextBefore = $state(null);
  let selected = $state(0);
  let failed = $state(0);
  let copied = $state(false);
  let editing = $state(false);
  let purging = $state(false);
  let word = $state('');
  let working = $state(false);
  let problem = $state('');
  let notice = $state('');
  let seq = 0;

  async function load() {
    const mine = ++seq;
    try {
      const result = await api('memory', { project, id });
      if (mine !== seq) return;
      const keep = data && data.current.id === result.current.id && selected < revisions.length - 1 ? revisions[selected].revision : null;
      data = result;
      revisions = [...result.revisions];
      nextBefore = result.next_before;
      const idx = keep ? revisions.findIndex((r) => r.revision === keep) : -1;
      selected = idx >= 0 ? idx : revisions.length - 1;
      failed = 0;
    } catch (error) {
      if (mine !== seq) return;
      if (error.status !== 401) failed = error.status === 404 ? 404 : 1;
    }
  }
  let loadingOlder = false;
  async function older() {
    if (loadingOlder) return;
    loadingOlder = true;
    const mine = seq;
    try {
      const page = await api('memory', { project, id, before: nextBefore });
      // A live reload in between replaced the list: drop this page.
      if (mine !== seq) return;
      revisions = [...page.revisions, ...revisions];
      selected += page.revisions.length;
      nextBefore = page.next_before;
    } catch { failed = 1; } finally { loadingOlder = false; }
  }
  async function copy() {
    try { await navigator.clipboard.writeText(data.current.id); copied = true; setTimeout(() => (copied = false), 1500); } catch { /* clipboard unavailable */ }
  }

  $effect(() => { project; id; untrack(() => { data = null; copied = false; editing = false; purging = false; problem = ''; notice = ''; load(); }); });
  $effect(() => { if (app.changed) untrack(load); });

  function saved(memory) {
    editing = false;
    notice = t('saved', memory.revision);
    setTimeout(() => (notice = ''), 4000);
    load();
  }
  function stop(reload) { editing = false; if (reload) load(); }
  // Archived by someone else while editing: close the editor and say why.
  $effect(() => { if (editing && data?.archived_at) untrack(() => { editing = false; problem = t('editArchived'); }); });

  // Archive/restore and purge. Failures keep the view and say why.
  async function act(view, body) {
    if (working) return null;
    working = true;
    problem = '';
    try {
      const response = await post(view, { project, id, ...body });
      if (response.status === 401) { app.session = null; return null; }
      if (!response.ok) {
        problem = response.status === 403 ? t('editDenied') : response.status === 404 ? t('notFound') : t('error');
        return null;
      }
      refreshShared(false);
      return await response.json();
    } catch {
      problem = t('error');
      return null;
    } finally {
      working = false;
    }
  }
  async function archive(value) {
    if (await act('archive', { archived: value })) {
      // The archived note says it already; a restore gets a short confirmation.
      notice = value ? '' : t('restoredNotice');
      if (!value) setTimeout(() => (notice = ''), 4000);
      // Reload the list and this detail now, without waiting for the live event.
      app.changed++;
    }
  }
  async function purge() {
    if (word.trim().toLowerCase() !== t('purgeWord')) return;
    if (await act('purge', { confirm: word.trim().toLowerCase() })) {
      go(listHref('', { archived: '1' }));
    }
  }

  const current = $derived(data?.current);
  const revision = $derived(revisions[selected]);
  const tree = $derived(revision ? parseMarkdown(revision.content) : []);
  const diff = $derived(selected > 0 ? lineDiff(revisions[selected - 1].content, revision.content) : null);
</script>

<article class="detail" aria-label={current ? heading(current) : t('memories')}>
  <a class="back" href={listHref(app.route.scoped ? project : '', app.route.filters)}>← {t('back')}</a>
  {#if failed}
    <div class="error" role="alert">{failed === 404 ? t('notFound') : t('error')}</div>
  {:else if current && revision}
    <div class="top">
      <span class="kind {revision.kind}">{t('kinds')[revision.kind] || revision.kind}</span>
      {#if current.topic}<span class="mono topic">{current.topic}</span>{/if}
      <span class="actions">
        {#if data.can_archive && !editing}
          <button type="button" class="btn" onclick={() => archive(!data.archived_at)} disabled={working}>
            {#if data.archived_at}
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 12a9 9 0 1 0 3-6.7L3 8"></path><path d="M3 3v5h5"></path></svg>{t('restore')}
            {:else}
              <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="4" width="18" height="5" rx="1.5"></rect><path d="M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9M10 13h4"></path></svg>{t('archive')}
            {/if}
          </button>
        {/if}
        {#if data.can_purge}
          <button type="button" class="btn danger" onclick={() => { problem = ''; purging = true; }}>{t('purge')}</button>
        {/if}
        {#if data.can_edit && !editing}
          <button type="button" class="btn" onclick={() => { selected = revisions.length - 1; notice = ''; editing = true; }}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16v4Z"></path><path d="m13.5 6.5 4 4"></path></svg>
            {t('edit')}
          </button>
        {/if}
        <button type="button" class="btn" onclick={copy}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="13" height="13" rx="3"></rect><path d="M4 16V5a2 2 0 0 1 2-2h11"></path></svg>
          {copied ? t('copied') : t('copy')}
        </button>
      </span>
    </div>

    {#if notice}<p class="saved" role="status">{notice}</p>{/if}
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    {#if data.archived_at}
      <p class="archived-note" role="note">{t('archivedSince', when(data.archived_at))}</p>
    {/if}
    <AlertDialog.Root bind:open={purging} onOpenChange={(open) => { if (open) word = ''; }}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay class="dialog-overlay" />
        <AlertDialog.Content class="dialog purge">
          <form onsubmit={(e) => { e.preventDefault(); purge(); }}>
            <AlertDialog.Title class="purge-title">{t('purgeTitle')}</AlertDialog.Title>
            <AlertDialog.Description class="purge-help">{t('purgeHelp')}</AlertDialog.Description>
            <label class="purge-field">
              <span>{t('purgeType', t('purgeWord'))}</span>
              <input class="purge-input" bind:value={word} autocomplete="off" spellcheck="false" />
            </label>
            {#if problem}<p class="purge-problem" role="alert">{problem}</p>{/if}
            <div class="purge-actions">
              <AlertDialog.Cancel class="btn">{t('cancel')}</AlertDialog.Cancel>
              <button type="submit" class="btn danger solid" disabled={working || word.trim().toLowerCase() !== t('purgeWord')}>{t('purgeConfirm')}</button>
            </div>
          </form>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>

    {#if editing}
      <MemoryEditor {project} memory={current} latest={current.revision} onsaved={saved} oncancel={stop} />
    {:else}
    <h2 class="serif">{heading(revision.title ? revision : current)}</h2>

    <dl class="facts">
      <div><dt>{t('state')}</dt><dd class="state {revision.state}">{t('states')[revision.state] || revision.state}</dd></div>
      <div><dt>{t('revision')}</dt><dd class="mono">{revision.revision}{#if revision.revision === current.revision}<span class="muted">{` · ${t('current')}`}</span>{/if}</dd></div>
      <div><dt>{t('source')}</dt><dd class="agent" title={revision.agent}><AgentIcon agent={revision.agent} label={false} /><span class="agent-name">{revision.agent}</span></dd></div>
      <div><dt>{t('updated')}</dt><dd title={stamp(revision.saved_at)}>{when(revision.saved_at)}</dd></div>
    </dl>

    <div class="md">{#each tree as node}<MdNode {node} />{/each}</div>
    {/if}

    <section class="history" aria-labelledby="history-title">
      <h3 id="history-title" class="label">{t('history')}</h3>
      <ol class:multi={revisions.length > 1}>
        {#each [...revisions.keys()].reverse() as n (revisions[n].revision)}
          {@const r = revisions[n]}
          <li aria-current={n === selected ? 'true' : undefined}>
            <span class="mono rev">rev {r.revision}</span>
            <span class="who"><AgentIcon agent={r.agent} size={18} label={false} />{author(r)} · {r.agent}</span>
            <span class="muted when" title={stamp(r.saved_at)}>{when(r.saved_at)}</span>
            {#if n !== selected}<button type="button" class="btn small" onclick={() => (selected = n)}>{t('view')}</button>{/if}
          </li>
        {/each}
      </ol>
      {#if nextBefore}<button type="button" class="btn" onclick={older}>{t('older')}</button>{/if}
      {#if diff}
        <div class="diff" aria-label={t('compare', revisions[selected - 1].revision, revision.revision)}>
          <div class="diff-head mono">{t('compare', revisions[selected - 1].revision, revision.revision)}</div>
          {#each diff as line}
            <div class="l {line.op === '-' ? 'del' : line.op === '+' ? 'add' : 'same'}">{line.op === ' ' ? '  ' : line.op === '-' ? '− ' : '+ '}{line.t}</div>
          {/each}
        </div>
      {:else if revision.revision > 1}
        <p class="muted">{t('needOlder')}</p>
      {/if}
    </section>

    <dl class="meta">
      <dt>{t('project')}</dt><dd class="mono"><a href={listHref(project, {})}>{project}</a></dd>
      <dt>{t('principal')}</dt><dd>{author(revision)}</dd>
      <dt>{t('created')}</dt><dd>{stamp(data.created_at)}</dd>
      <dt>{t('id')}</dt><dd class="mono">{current.id}</dd>
    </dl>
  {:else}
    <div class="skeleton" aria-busy="true"></div>
  {/if}
</article>

<style>
  .detail { min-width: 0; flex: 1; padding: 36px 40px 40px; display: flex; flex-direction: column; gap: 22px; background: var(--surface);
    border: 1px solid var(--line); border-radius: 20px; box-shadow: var(--shadow); }
  /* The card grows with its content (the column scrolls); at least one screen tall. */
  @media (min-width: 1181px) { .detail { align-self: flex-start; min-height: calc(100vh - 40px); } }
  .back { display: none; font-size: 13px; text-decoration: none; }
  .top { display: flex; align-items: center; gap: 8px; }
  .topic { font-size: 12px; color: var(--muted); min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .actions { margin-left: auto; display: flex; gap: 6px; flex: none; }
  svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  h2 { margin: 0; font-size: 34px; line-height: 1.15; letter-spacing: -0.01em; overflow-wrap: anywhere; }
  .facts { margin: 0; display: grid; grid-template-columns: max-content max-content minmax(0, 1fr) max-content; gap: 16px 32px; padding: 16px 0;
    border-top: 1px solid var(--line); border-bottom: 1px solid var(--line); }
  .facts dt, .meta dt { font-size: 11px; color: var(--muted); text-transform: uppercase; letter-spacing: .07em; }
  .facts dd { margin: 4px 0 0; font-weight: 550; }
  .facts .agent { display: flex; align-items: center; gap: 8px; min-width: 0; white-space: nowrap; }
  .facts .agent :global(.agent) { flex: none; }
  .facts .agent-name { overflow: hidden; text-overflow: ellipsis; }
  .md { font-size: 16px; line-height: 1.7; color: var(--body); max-width: 68ch; overflow-wrap: anywhere; }
  .md :global(p) { margin: 0 0 1em; }
  .md :global(h1), .md :global(h2), .md :global(h3), .md :global(h4) { font-family: var(--serif); font-weight: 500; margin: 1.2em 0 .4em; line-height: 1.2; }
  .md :global(code) { font: 13px var(--mono); background: var(--tint); padding: 1px 5px; border-radius: 5px; }
  .md :global(pre) { overflow-x: auto; padding: 14px 16px; border-radius: 12px; background: var(--tint); }
  .md :global(pre code) { padding: 0; background: none; }
  .md :global(ul), .md :global(ol) { padding-left: 1.3em; margin: 0 0 1em; }
  .history { display: flex; flex-direction: column; gap: 12px; padding: 18px 20px; border-radius: 14px; background: var(--tint); }
  .history ol { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .history li { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
  .history li[aria-current="true"] .rev { font-weight: 600; }
  .history ol.multi li[aria-current="true"] { background: var(--hover); border-radius: 10px; }
  .rev { width: 48px; font-size: 12px; color: var(--accent); }
  .who { flex: 1 1 160px; display: flex; align-items: center; gap: 8px; min-width: 0; }
  .when { font-size: 12px; }
  .history .btn.small { width: auto; height: 28px; padding: 0 10px; font-size: 12px; }
  .diff { border-radius: 12px; background: var(--surface); border: 1px solid var(--line); overflow-x: auto; font: 12px/1.6 var(--mono); }
  .diff-head { padding: 8px 12px; border-bottom: 1px solid var(--line); color: var(--muted); }
  .l { padding: 0 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .l.add { background: var(--k-progress-bg); color: var(--k-progress); }
  .l.del { background: var(--danger-soft); color: var(--danger); }
  .meta { margin: 0; display: grid; grid-template-columns: auto 1fr; gap: 6px 16px; font-size: 13px; }
  .meta dd { margin: 0; overflow-wrap: anywhere; }
  .problem { margin: 0; padding: 10px 14px; border-radius: 10px; background: var(--danger-soft); color: var(--danger); }
  .archived-note { margin: 0; padding: 10px 14px; border-radius: 10px; background: var(--warn-soft); border: 1px solid var(--warn-line); color: var(--warn); }
  .btn.danger { color: var(--danger); }
  .btn.danger.solid { background: var(--danger); border-color: var(--danger); color: var(--surface); font-weight: 600; }
  .btn.danger.solid:disabled { opacity: .45; }
  /* Purge confirmation: modal destructive dialog (portaled, so global selectors). */
  :global(.dialog-overlay) { position: fixed; inset: 0; z-index: 50; background: rgba(20, 12, 16, .45); backdrop-filter: blur(2px); }
  :global(.dialog.purge) { position: fixed; z-index: 51; left: 50%; top: 50%; transform: translate(-50%, -50%);
    width: min(440px, calc(100vw - 32px)); max-height: calc(100vh - 32px); overflow-y: auto; box-sizing: border-box;
    padding: 24px; border-radius: 18px; background: var(--surface); border: 1px solid var(--line); box-shadow: var(--shadow); }
  :global(.dialog.purge form) { display: flex; flex-direction: column; gap: 14px; }
  :global(.dialog.purge .purge-title) { margin: 0; font: 500 22px var(--serif); color: var(--danger); }
  :global(.dialog.purge .purge-help) { margin: 0; color: var(--body); line-height: 1.55; }
  :global(.dialog.purge .purge-field) { display: flex; flex-direction: column; gap: 6px; font-size: 14px; color: var(--ink-2); }
  :global(.dialog.purge .purge-input) { font: 14px var(--mono); color: var(--ink); background: var(--surface); border: 1px solid var(--line-strong);
    border-radius: 10px; padding: 9px 12px; }
  :global(.dialog.purge .purge-input:focus-visible) { outline: 2px solid var(--danger); outline-offset: 1px; }
  :global(.dialog.purge .purge-problem) { margin: 0; padding: 8px 12px; border-radius: 10px; background: var(--danger-soft); color: var(--danger); }
  :global(.dialog.purge .purge-actions) { display: flex; justify-content: flex-end; gap: 8px; flex-wrap: wrap; }
  .saved { margin: 0; padding: 10px 14px; border-radius: 10px; background: var(--live-soft); color: var(--live); }
  .skeleton { height: 240px; border-radius: 14px; background: var(--tint); }
  @media (max-width: 960px) {
    .detail { padding: 24px 20px; border-radius: 0; border: 0; box-shadow: none; }
    .back { display: inline; }
    .facts { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .top { flex-wrap: wrap; }
    .actions { flex-wrap: wrap; }
    h2 { font-size: 28px; }
  }
</style>
