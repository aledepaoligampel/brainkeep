<script>
  // "Share" for one of the session's own projects: members with their role, adding a
  // user, changing a role and removing someone after a destructive confirmation.
  import { Dialog, AlertDialog, Select } from 'bits-ui';
  import Person from './lib/Person.svelte';
  import { app, t, api, send } from './lib/state.svelte.js';

  let { project } = $props();

  let data = $state(null); // {project, members:[{handle,name,picture,role,disabled}], people:[…]}
  let open = $state(false);
  let pick = $state('');
  let role = $state('read');
  let problem = $state('');
  let working = $state(false);
  let removing = $state(null);
  let removeOpen = $state(false);

  async function load() {
    const asked = project;
    try {
      const found = await api('sharing', { project: asked });
      // A late answer for another project must not replace the current one.
      if (asked === project) data = found;
    } catch { if (asked === project) data = null; }
  }
  $effect(() => { project; app.changed; load(); });

  const candidates = $derived((data?.people || []).filter((p) => !(data?.members || []).some((m) => m.handle === p.handle)));
  const items = $derived(candidates.map((p) => ({ value: p.handle, label: p.name ? `${p.name} (${p.handle})` : p.handle })));
  const chosen = $derived(candidates.find((p) => p.handle === pick));

  function serverError(body) { return (body && t('serverErrors')[body.error]) || t('actionFailed'); }
  async function change(view, body) {
    working = true; problem = '';
    const r = await send(view, { project, ...body });
    working = false;
    if (!r.ok) { problem = serverError(r.body); return false; }
    await load();
    return true;
  }
  async function add(event) {
    event.preventDefault();
    if (!pick) return;
    if (await change('share', { handle: pick, role })) { pick = ''; role = 'read'; }
  }
  async function remove() {
    if (await change('unshare', { handle: removing.handle })) removeOpen = false;
  }
  function ask(member) { problem = ''; removing = member; removeOpen = true; }
  const me = $derived(app.session?.user || '');
</script>

{#if data}
  <div class="share-row">
    <button type="button" class="btn" onclick={() => { problem = ''; open = true; }}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="9" cy="8" r="3.5"></circle><path d="M2.5 20a6.5 6.5 0 0 1 13 0"></path><path d="M19 8v6M16 11h6"></path></svg>
      {t('share')}
    </button>
    {#if data.members.length}
      <span class="faces" role="img" aria-label={t('sharedCount', data.members.map((m) => m.handle).join(', '))}>
        {#each data.members.slice(0, 5) as m (m.handle)}<Person user={m} small status={m.disabled ? 'disabled' : 'active'} />{/each}
      </span>
    {/if}
  </div>

  <Dialog.Root bind:open>
    <Dialog.Portal>
      <Dialog.Overlay class="dialog-overlay" />
      <Dialog.Content class="modal wide">
        <div class="modal-head">
          <div>
            <Dialog.Title class="modal-title">{t('shareTitle', project)}</Dialog.Title>
            <Dialog.Description class="modal-desc">{t('shareHelp', `${me}/${project}`)}</Dialog.Description>
          </div>
          <Dialog.Close class="icon-btn" aria-label={t('close')}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"></path></svg>
          </Dialog.Close>
        </div>

        {#if candidates.length}
          <form class="add" onsubmit={add}>
            <div class="field grow">
              <span class="field-label" id="share-person">{t('person')}</span>
              <Select.Root type="single" value={pick} {items} onValueChange={(v) => (pick = v)}>
                <Select.Trigger class="field-input select" aria-labelledby="share-person">
                  {#if chosen}<Person user={chosen} small /><span class="pick-text">{items.find((i) => i.value === pick)?.label}</span>
                  {:else}<span class="placeholder">{t('pickPerson')}</span>{/if}
                  <svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
                </Select.Trigger>
                <Select.Portal>
                  <Select.Content class="menu pick" align="start" sideOffset={6} collisionPadding={12}>
                    <Select.Viewport class="pick-list">
                      {#each candidates as p (p.handle)}
                        <Select.Item class="menu-item" value={p.handle} label={p.name ? `${p.name} (${p.handle})` : p.handle}>
                          {#snippet children({ selected })}
                            <Person user={p} small />
                            <span class="pick-text">{p.name || p.handle}{#if p.name} <span class="mono muted">{p.handle}</span>{/if}</span>
                            {#if selected}<svg class="check" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5 10 17 19 7"></path></svg>{/if}
                          {/snippet}
                        </Select.Item>
                      {/each}
                    </Select.Viewport>
                  </Select.Content>
                </Select.Portal>
              </Select.Root>
            </div>
            <div class="seg tall" role="group" aria-label={t('role')}>
              {#each ['read', 'write'] as r}
                <button type="button" aria-pressed={role === r} onclick={() => (role = r)}>{t('roles')[r]}</button>
              {/each}
            </div>
            <button type="submit" class="btn primary tall" disabled={!pick || working}>{t('add')}</button>
          </form>
        {:else}
          <p class="field-help">{t('noPeople')}</p>
        {/if}

        {#if problem && !removeOpen}<p class="form-error" role="alert">{problem}</p>{/if}

        <div class="field">
          <span class="label">{t('sharedWith')}</span>
          {#if data.members.length}
            <ul class="members">
              {#each data.members as m (m.handle)}
                <li>
                  <Person user={m} status={m.disabled ? 'disabled' : 'active'} />
                  <span class="who">
                    {#if m.name}<span class="name">{m.name}</span>{/if}
                    <span class="mono handle" class:muted={Boolean(m.name)} class:solo={!m.name}>{m.handle}</span>
                  </span>
                  <span class="seg" role="group" aria-label={t('roleOf', m.handle)}>
                    {#each ['read', 'write'] as r}
                      <button type="button" aria-pressed={m.role === r} disabled={working}
                        onclick={() => { if (m.role !== r) change('share', { handle: m.handle, role: r }); }}>{t('roles')[r]}</button>
                    {/each}
                  </span>
                  <button type="button" class="icon-btn danger" aria-label={t('removeMember', m.handle)} onclick={() => ask(m)}>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"></path></svg>
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="field-help">{t('noMembers')}</p>
          {/if}
          <p class="field-help">{t('shareNote')}</p>
        </div>

        <AlertDialog.Root bind:open={removeOpen}>
          <AlertDialog.Portal>
            <AlertDialog.Overlay class="dialog-overlay" />
            <AlertDialog.Content class="modal small">
              {#if removing}
                <div class="title-row">
                  <span class="badge-icon danger" aria-hidden="true">
                    <svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"></path></svg>
                  </span>
                  <AlertDialog.Title class="modal-title">{t('removeTitle', removing.handle, project)}</AlertDialog.Title>
                </div>
                <AlertDialog.Description class="modal-desc">{t('removeHelp')}</AlertDialog.Description>
                {#if problem}<p class="form-error" role="alert">{problem}</p>{/if}
                <div class="modal-actions">
                  <AlertDialog.Cancel class="btn tall">{t('cancel')}</AlertDialog.Cancel>
                  <button type="button" class="btn danger solid tall" disabled={working} onclick={remove}>{t('removeConfirm')}</button>
                </div>
              {/if}
            </AlertDialog.Content>
          </AlertDialog.Portal>
        </AlertDialog.Root>
      </Dialog.Content>
    </Dialog.Portal>
  </Dialog.Root>
{/if}

<style>
  .share-row { display: flex; align-items: center; gap: 10px; }
  .share-row svg { width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .faces { display: inline-flex; }
  .faces :global(.person-avatar + .person-avatar) { margin-left: -6px; }
  .add { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 8px; }
  .grow { flex: 1 1 220px; }
  .placeholder { color: var(--muted); }
  .members { list-style: none; margin: 0; padding: 0; border: 1px solid var(--line); border-radius: 14px; overflow: hidden; }
  .members li { display: flex; align-items: center; gap: 12px; padding: 10px 10px 10px 14px; border-bottom: 1px solid var(--line); }
  .members li:last-child { border-bottom: 0; }
  .who { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .name { font: 600 14px/1.25 var(--sans); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .handle { font-size: 12px; }
  .handle.solo { font-size: 14px; color: var(--ink); }
  @media (max-width: 560px) {
    .members li { flex-wrap: wrap; }
    .members .seg { order: 3; margin-left: 46px; }
  }
</style>
