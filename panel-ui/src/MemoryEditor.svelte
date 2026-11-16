<script>
  // Edits a memory as a new revision (`correct` with expected_revision). On a conflict
  // both versions are shown and the user chooses; nothing is overwritten silently.
  import { Select } from 'bits-ui';
  import { lineDiff } from './lib/markdown.js';
  import { KINDS } from './lib/i18n.js';
  import { t, post, api, app } from './lib/state.svelte.js';

  let { project, memory, latest, onsaved, oncancel } = $props();

  // The draft starts from the revision being edited and keeps it as its base.
  let base = $state(memory);
  let draft = $state({
    title: memory.title || '', kind: memory.kind, topic: memory.topic || '',
    confirmed: memory.state === 'confirmed', content: memory.content,
  });
  let saving = $state(false);
  let problem = $state('');
  let theirs = $state(null); // newer saved revision, while resolving a conflict
  let operation = crypto.randomUUID();

  const kindItems = $derived(KINDS.map((k) => ({ value: k, label: t('kinds')[k] })));
  const stateItems = $derived([
    { value: 'confirmed', label: t('states').confirmed }, { value: 'proposed', label: t('states').proposed },
  ]);
  const topicFixed = $derived(Boolean(base.topic));
  const dirty = $derived(
    draft.content !== base.content || draft.title !== (base.title || '') || draft.kind !== base.kind
    || draft.topic !== (base.topic || '') || draft.confirmed !== (base.state === 'confirmed'),
  );
  // The detail reloads live; a newer revision means saving will conflict.
  const newer = $derived(latest > base.revision);
  const diff = $derived(theirs ? lineDiff(theirs.content, draft.content) : null);

  // The server counts UTF-8 bytes; textarea maxlength counts UTF-16 units.
  const tooLong = $derived(new TextEncoder().encode(draft.content).length > 16384);

  async function save() {
    if (saving || !draft.content.trim()) return;
    if (tooLong) { problem = t('editTooLong'); return; }
    saving = true;
    problem = '';
    // An empty title keeps the current one (titles cannot be cleared).
    const body = {
      project, id: base.id, expected_revision: base.revision, operation_id: operation,
      content: draft.content, title: draft.title.trim(), kind: draft.kind, confirmed: draft.confirmed,
    };
    if (!topicFixed && draft.topic.trim()) body.topic = draft.topic.trim();
    try {
      const response = await post('correct', body);
      if (response.ok) { onsaved(await response.json()); return; }
      if (response.status === 401) { app.session = null; return; }
      if (response.status === 409) { await conflict(); return; }
      const detail = await response.json().catch(() => ({}));
      problem = response.status === 403 ? t('editDenied')
        : detail.error?.startsWith('secret') ? t('editSecret')
        : detail.error?.startsWith('topic') ? t('editTopic')
        : response.status === 400 ? t('editInvalid') : t('error');
    } catch {
      problem = t('error');
    } finally {
      saving = false;
    }
  }
  async function conflict() {
    try {
      const fresh = await api('memory', { project, id: base.id });
      // Archived meanwhile: there is nothing to choose; restoring comes first.
      if (fresh.archived_at) { problem = t('editArchived'); return; }
      theirs = fresh.current;
    } catch {
      problem = t('error');
    }
  }
  // Keep mine: the draft now replaces the newer revision, as a fresh operation.
  function keepMine() {
    base = theirs;
    theirs = null;
    operation = crypto.randomUUID();
    save();
  }
  function keepTheirs() { theirs = null; oncancel(true); }
  function keydown(event) {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') { event.preventDefault(); save(); }
    if (event.key === 'Escape' && !theirs && !dirty) oncancel(false);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form class="editor" aria-label={t('editTitle')} onsubmit={(e) => { e.preventDefault(); save(); }} onkeydown={keydown}>
  <div class="grid">
    <label class="field wide">
      <span class="label">{t('title')}</span>
      <input class="input" bind:value={draft.title} maxlength="160" autocomplete="off" />
    </label>
    <div class="field">
      <span class="label" id="kind-label">{t('kind')}</span>
      <Select.Root type="single" value={draft.kind} items={kindItems} onValueChange={(v) => (draft.kind = v)}>
        <Select.Trigger class="input select" aria-labelledby="kind-label">
          <span class="dot {draft.kind}"></span>{t('kinds')[draft.kind]}
          <svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
        </Select.Trigger>
        <Select.Portal>
          <Select.Content class="menu pick" align="start" sideOffset={6} collisionPadding={12}>
            <Select.Viewport class="pick-list">
              {#each kindItems as option (option.value)}
                <Select.Item class="menu-item" value={option.value} label={option.label}>
                  {#snippet children({ selected })}
                    <span class="dot {option.value}"></span><span class="pick-text">{option.label}</span>
                    {#if selected}<svg class="check" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5 10 17 19 7"></path></svg>{/if}
                  {/snippet}
                </Select.Item>
              {/each}
            </Select.Viewport>
          </Select.Content>
        </Select.Portal>
      </Select.Root>
    </div>
    <div class="field">
      <span class="label" id="state-label">{t('state')}</span>
      <Select.Root type="single" value={draft.confirmed ? 'confirmed' : 'proposed'} items={stateItems}
        onValueChange={(v) => (draft.confirmed = v === 'confirmed')}>
        <Select.Trigger class="input select" aria-labelledby="state-label">
          {draft.confirmed ? t('states').confirmed : t('states').proposed}
          <svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
        </Select.Trigger>
        <Select.Portal>
          <Select.Content class="menu pick" align="start" sideOffset={6} collisionPadding={12}>
            <Select.Viewport class="pick-list">
              {#each stateItems as option (option.value)}
                <Select.Item class="menu-item" value={option.value} label={option.label}>
                  {#snippet children({ selected })}
                    <span class="pick-text">{option.label}</span>
                    {#if selected}<svg class="check" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5 10 17 19 7"></path></svg>{/if}
                  {/snippet}
                </Select.Item>
              {/each}
            </Select.Viewport>
          </Select.Content>
        </Select.Portal>
      </Select.Root>
    </div>
    <label class="field wide">
      <span class="label">{t('topic')}</span>
      <input class="input mono" bind:value={draft.topic} maxlength="128" autocomplete="off" readonly={topicFixed}
        aria-describedby={topicFixed ? 'topic-help' : undefined} />
      {#if topicFixed}<span class="help" id="topic-help">{t('topicFixed')}</span>{/if}
    </label>
    <label class="field full">
      <span class="label">{t('content')}</span>
      <textarea class="input mono" bind:value={draft.content} rows="14" maxlength="16384" required></textarea>
    </label>
  </div>

  {#if theirs}
    <section class="conflict" role="alert" aria-labelledby="conflict-title">
      <h3 id="conflict-title">{t('conflictTitle', theirs.revision)}</h3>
      <p class="muted">{t('conflictHelp')}</p>
      {#if theirs.topic && theirs.topic !== draft.topic.trim()}
        <p class="muted">{t('conflictTopic', theirs.topic)}</p>
      {/if}
      <div class="diff" aria-label={t('conflictDiff', theirs.revision)}>
        <div class="diff-head mono">{t('conflictDiff', theirs.revision)}</div>
        {#each diff as line}
          <div class="l {line.op === '-' ? 'del' : line.op === '+' ? 'add' : 'same'}">{line.op === ' ' ? '  ' : line.op === '-' ? '− ' : '+ '}{line.t}</div>
        {/each}
      </div>
      <div class="row">
        <button type="button" class="btn primary" onclick={keepMine} disabled={saving}>{t('keepMine')}</button>
        <button type="button" class="btn" onclick={keepTheirs}>{t('keepTheirs')}</button>
      </div>
    </section>
  {:else}
    {#if newer}<p class="notice" role="status">{t('editNewer')}</p>{/if}
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    <div class="row">
      <button type="submit" class="btn primary" disabled={saving || !dirty || !draft.content.trim()}>
        {saving ? t('saving') : t('save')}
      </button>
      <button type="button" class="btn" onclick={() => oncancel(false)}>{t('cancel')}</button>
      <span class="muted hint">{t('saveHint')}</span>
    </div>
  {/if}
</form>

<style>
  .editor { display: flex; flex-direction: column; gap: 18px; }
  .grid { display: grid; grid-template-columns: minmax(0, 2fr) minmax(0, 1fr) minmax(0, 1fr); gap: 14px 16px; }
  .field { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
  .field.full { grid-column: 1 / -1; }
  .field.wide:last-of-type { grid-column: 1 / -1; }
  .label { font-size: 11px; color: var(--muted); text-transform: uppercase; letter-spacing: .07em; }
  .editor :global(.input) { font: inherit; color: var(--ink); background: var(--surface); border: 1px solid var(--line-strong); border-radius: 10px;
    padding: 8px 12px; min-height: 38px; width: 100%; box-sizing: border-box; }
  .editor :global(.input:focus-visible) { outline: 2px solid var(--accent); outline-offset: 1px; }
  .editor :global(.input[readonly]) { background: var(--tint); color: var(--muted); }
  .editor :global(textarea.input) { resize: vertical; line-height: 1.6; font-size: 13px; }
  .editor :global(.input.mono) { font-family: var(--mono); }
  .editor :global(.input.select) { display: flex; align-items: center; gap: 8px; cursor: pointer; text-align: left; }
  .editor :global(.input.select .chev) { margin-left: auto; width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 2; }
  .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; background: var(--k-note); }
  .dot.decision { background: var(--k-decision); } .dot.progress { background: var(--k-progress); }
  .dot.discovery { background: var(--k-discovery); } .dot.preference { background: var(--k-preference); }
  .help { font-size: 12px; color: var(--muted); }
  .row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .hint { font-size: 12px; margin-left: auto; }
  .notice { margin: 0; padding: 10px 14px; border-radius: 10px; background: var(--warn-soft); border: 1px solid var(--warn-line); color: var(--warn); }
  .problem { margin: 0; padding: 10px 14px; border-radius: 10px; background: var(--danger-soft); color: var(--danger); }
  .conflict { display: flex; flex-direction: column; gap: 12px; padding: 18px 20px; border-radius: 14px;
    background: var(--warn-soft); border: 1px solid var(--warn-line); }
  .conflict h3 { margin: 0; font: 500 18px var(--serif); color: var(--ink); }
  .conflict p { margin: 0; }
  .diff { border-radius: 12px; background: var(--surface); border: 1px solid var(--line); overflow-x: auto; font: 12px/1.6 var(--mono); max-height: 360px; overflow-y: auto; }
  .diff-head { padding: 8px 12px; border-bottom: 1px solid var(--line); color: var(--muted); }
  .l { padding: 0 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .l.add { background: var(--k-progress-bg); color: var(--k-progress); }
  .l.del { background: var(--danger-soft); color: var(--danger); }
  @media (max-width: 960px) {
    .grid { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
    .field.wide { grid-column: 1 / -1; }
    .hint { display: none; }
  }
</style>
