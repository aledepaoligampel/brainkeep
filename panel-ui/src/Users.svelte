<script>
  // User administration (admin only): list, invite, disable, enable and regenerate the
  // personal token. A token is shown once, in the dialog that follows the action.
  import { onMount } from 'svelte';
  import { DropdownMenu, Dialog, AlertDialog } from 'bits-ui';
  import { inviteMessage } from './lib/invite.js';
  import PageHeader from './lib/PageHeader.svelte';
  import Person from './lib/Person.svelte';
  import { app, t, api, send, stamp } from './lib/state.svelte.js';

  let users = $state(null);
  let failed = $state(0); // HTTP status of a failed load, 0 when fine
  let filter = $state('all');

  let inviting = $state(false);
  let email = $state('');
  let handle = $state('');
  let handleEdited = $state(false);
  let problem = $state('');
  let working = $state(false);

  let issued = $state(null); // {kind, handle, email, token}
  let confirm = $state(null); // {kind: 'disable' | 'regenerate', user}
  let confirmOpen = $state(false);
  let issuedOpen = $state(false);
  let copied = $state('');

  async function load() {
    try {
      users = (await api('users')).users;
      failed = 0;
    } catch (error) {
      failed = error.status || 503;
    }
  }
  onMount(load);

  const stateOf = (u) => (u.admin ? 'admin' : u.disabled ? 'disabled' : u.signed_in ? 'active' : 'pending');
  const counts = $derived.by(() => {
    const c = { all: 0, active: 0, pending: 0, disabled: 0 };
    for (const u of users || []) {
      c.all += 1;
      const s = stateOf(u);
      if (s === 'admin' || s === 'active') c.active += 1;
      else c[s] += 1;
    }
    return c;
  });
  const shown = $derived((users || []).filter((u) => {
    const s = stateOf(u);
    return filter === 'all' || s === filter || (filter === 'active' && s === 'admin');
  }));
  function accessOf(u) {
    if (u.disabled) return [t('access').none];
    const list = [];
    if (app.google && (u.email || u.admin)) list.push(t('access').google);
    if (u.admin) list.push(t('access').adminKey);
    else if (u.has_token) list.push(t('access').token);
    return list.length ? list : [t('access').none];
  }
  function serverError(body) {
    return (body && t('serverErrors')[body.error]) || t('actionFailed');
  }

  // The handle follows the email until edited: local part, lowercased, invalid runs as "-".
  function suggest(value) {
    return value.split('@')[0].toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[^a-z0-9]+/, '').slice(0, 32);
  }
  function onEmail() { if (!handleEdited) handle = suggest(email); }
  function openInvite() {
    email = ''; handle = ''; handleEdited = false; problem = ''; inviting = true;
  }

  async function invite(event) {
    event.preventDefault();
    working = true; problem = '';
    const r = await send('invite', { email: email.trim(), handle: handle.trim() });
    working = false;
    if (!r.ok) { problem = serverError(r.body); return; }
    inviting = false;
    show('invited', r.body);
    load();
  }
  async function enable(user) {
    const r = await send('enable', { handle: user.handle });
    if (!r.ok) { problem = serverError(r.body); return; }
    show('enabled', { ...r.body, email: user.email });
    load();
  }
  async function confirmed() {
    const { kind, user } = confirm;
    working = true; problem = '';
    const r = await send(kind === 'regenerate' ? 'token' : kind, { handle: user.handle });
    working = false;
    if (!r.ok) { problem = serverError(r.body); return; }
    confirmOpen = false;
    if (kind === 'regenerate') show('token', { ...r.body, email: user.email });
    load();
  }
  function ask(kind, user) { problem = ''; confirm = { kind, user }; confirmOpen = true; }
  function show(kind, body) {
    issued = { kind, handle: body.handle, email: body.email, token: body.token };
    copied = '';
    revealed = false;
    issuedOpen = true;
  }
  function closeIssued(open) {
    // The token leaves memory with the dialog; it is never shown again.
    if (!open) issued = null;
  }

  // With Google the invitation needs no token: the person signs in and connects each agent
  // with OAuth from "Connect agents". The token stays here, hidden, for when Google is not an option.
  const viaGoogle = $derived(Boolean(issued && app.google && issued.email));
  let revealed = $state(false);
  const message = $derived(issued
    ? inviteMessage(t, location.origin, { google: app.google, email: issued.email, token: issued.token })
    : '');
  async function copy(text, key) {
    try { await navigator.clipboard.writeText(text); copied = key; setTimeout(() => { if (copied === key) copied = ''; }, 2000); }
    catch { copied = ''; }
  }
  const titles = { invited: 'invitedTitle', token: 'tokenTitle', enabled: 'enabledTitle' };
  const filters = ['all', 'active', 'pending', 'disabled'];
</script>

<section class="page users" aria-labelledby="users-title">
  <PageHeader id="users-title" title={t('users')} description={t('usersHelp')}
    count={users ? t(users.length === 1 ? 'people1' : 'people', users.length) : ''}>
    {#snippet actions()}
      <button type="button" class="btn primary tall" onclick={openInvite}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v14M5 12h14"></path></svg>{t('invitePerson')}
      </button>
    {/snippet}
  </PageHeader>

  {#if !app.google}
    <p class="notice" role="note">
      <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M12 8v5M12 16.5v.01"></path></svg>
      <span>{t('noGoogle')}</span>
    </p>
  {/if}

  {#if failed}
    <div class="error" role="alert">{failed === 404 || failed === 403 ? t('notFound') : t('error')}</div>
  {:else if users}
    <div class="chips" role="group" aria-label={t('filterUsers')}>
      {#each filters as f}
        <button type="button" class="chip" aria-pressed={filter === f} onclick={() => (filter = f)}>
          {#if f !== 'all'}<span class="dot {f}" aria-hidden="true"></span>{/if}{t('userFilters')[f]}<span class="n">{counts[f]}</span>
        </button>
      {/each}
    </div>

    {#if problem && !confirmOpen && !inviting}<p class="form-error" role="alert">{problem}</p>{/if}

    <ul class="rows">
      <li class="row header" aria-hidden="true">
        {#each t('columns') as column}<span>{column}</span>{/each}<span></span>
      </li>
      {#each shown as u (u.handle)}
        {@const s = stateOf(u)}
        <li class="row" class:off={u.disabled}>
          <span class="person">
            <Person user={u} status={s} />
            <span class="who">
              <span class="name">{u.name || u.email || u.handle}</span>
              <span class="sub">{u.name ? u.email : s === 'pending' ? t('neverSignedIn') : ''}</span>
            </span>
          </span>
          <span class="mono handle">{u.handle}</span>
          <span class="access">{#each accessOf(u) as a}<span class="tag">{a}</span>{/each}</span>
          <span><span class="state-pill {s}">{t('userStates')[s]}</span></span>
          <span class="muted when">{stamp(u.created_at)}</span>
          <span class="act">
            {#if !u.admin}
              <DropdownMenu.Root>
                <DropdownMenu.Trigger class="icon-btn" aria-label={t('actionsFor', u.handle)}>
                  <svg viewBox="0 0 24 24" aria-hidden="true" class="dots"><circle cx="5" cy="12" r="1.6"></circle><circle cx="12" cy="12" r="1.6"></circle><circle cx="19" cy="12" r="1.6"></circle></svg>
                </DropdownMenu.Trigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.Content class="menu" align="end" sideOffset={6} collisionPadding={12}>
                    {#if u.disabled}
                      <DropdownMenu.Item class="menu-item" onSelect={() => enable(u)}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="8" cy="15" r="4"></circle><path d="m11 12 9-9M17 6l3 3M15 8l2 2"></path></svg>{t('enable')}
                      </DropdownMenu.Item>
                    {:else}
                      <DropdownMenu.Item class="menu-item" onSelect={() => ask('regenerate', u)}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="8" cy="15" r="4"></circle><path d="m11 12 9-9M17 6l3 3M15 8l2 2"></path></svg>{t('regenerate')}
                      </DropdownMenu.Item>
                    {/if}
                    {#if app.google && u.email && !u.disabled}
                      <!-- With Google the invitation has no token, so it can be copied again at any time. -->
                      <DropdownMenu.Item class="menu-item" onSelect={() => copy(inviteMessage(t, location.origin, { google: true, email: u.email }), 'invite')}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16v12H4z"></path><path d="m4 7 8 6 8-6"></path></svg>{t('copyInvite')}
                      </DropdownMenu.Item>
                    {/if}
                    {#if u.email}
                      <DropdownMenu.Item class="menu-item" onSelect={() => copy(u.email, 'email')}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"></rect><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"></path></svg>{t('copyEmail')}
                      </DropdownMenu.Item>
                    {/if}
                    {#if !u.disabled || (!u.has_data && !u.admin)}<div class="menu-sep"></div>{/if}
                    {#if !u.disabled}
                      <DropdownMenu.Item class="menu-item danger" onSelect={() => ask('disable', u)}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M5.6 5.6l12.8 12.8"></path></svg>{t('disable')}
                      </DropdownMenu.Item>
                    {/if}
                    {#if !u.has_data && !u.admin}
                      <!-- Only people who never stored anything; the others are disabled instead. -->
                      <DropdownMenu.Item class="menu-item danger" onSelect={() => ask('delete', u)}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"></path></svg>{t('deleteUser')}
                      </DropdownMenu.Item>
                    {/if}
                  </DropdownMenu.Content>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
            {/if}
          </span>
        </li>
      {/each}
    </ul>
    <p class="muted console">{t('consoleHint')} <code class="mono">brainkeep users list</code></p>
  {/if}
</section>

<Dialog.Root bind:open={inviting}>
  <Dialog.Portal>
    <Dialog.Overlay class="dialog-overlay" />
    <Dialog.Content class="modal">
      <div class="modal-head">
        <div>
          <Dialog.Title class="modal-title">{t('invitePerson')}</Dialog.Title>
          <Dialog.Description class="modal-desc">{t('inviteHelp')}</Dialog.Description>
        </div>
        <Dialog.Close class="icon-btn" aria-label={t('close')}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"></path></svg>
        </Dialog.Close>
      </div>
      <form class="invite" onsubmit={invite}>
        <div class="field">
          <label class="field-label" for="invite-email">{t('email')}</label>
          <input id="invite-email" class="field-input" type="email" required autocomplete="off" spellcheck="false"
            bind:value={email} oninput={onEmail}>
          <p class="field-help">{app.google ? t('emailHelpGoogle') : t('emailHelp')}</p>
        </div>
        <div class="field">
          <label class="field-label" for="invite-handle">{t('handle')}</label>
          <input id="invite-handle" class="field-input mono" required maxlength="32" autocomplete="off" spellcheck="false"
            bind:value={handle} oninput={() => (handleEdited = true)}>
          <p class="field-help">{t('handleHelp', handle || t('handleExample'))}</p>
        </div>
        {#if problem}<p class="form-error" role="alert">{problem}</p>{/if}
        <div class="modal-actions">
          <Dialog.Close class="btn tall">{t('cancel')}</Dialog.Close>
          <button type="submit" class="btn primary tall" disabled={working}>{working ? t('working') : t('inviteSubmit')}</button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

{#if copied === 'invite' || copied === 'email'}
  <p class="toast" role="status">{t(copied === 'invite' ? 'inviteCopied' : 'emailCopied')}</p>
{/if}

<Dialog.Root bind:open={issuedOpen} onOpenChange={closeIssued}>
  <Dialog.Portal>
    <Dialog.Overlay class="dialog-overlay" />
    <Dialog.Content class="modal">
      {#if issued}
        <div class="title-row">
          <span class="badge-icon ok" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 12.5 4.5 4.5L19 7.5"></path></svg></span>
          <Dialog.Title class="modal-title">{t(titles[issued.kind], issued.handle)}</Dialog.Title>
        </div>
        <div class="field">
          <span class="label">{t('personalToken')}</span>
          {#if viaGoogle && !revealed}
            <p class="field-help">{t('tokenOptional')}</p>
            <button type="button" class="btn reveal" onclick={() => (revealed = true)}>{t('showToken')}</button>
          {:else}
          <div class="secret">
            <code>{issued.token}</code>
            <button type="button" class="btn" onclick={() => copy(issued.token, 'token')}>
              <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"></rect><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"></path></svg>
              {copied === 'token' ? t('copiedShort') : t('copyShort')}
            </button>
          </div>
          <Dialog.Description class="warn-note">
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M12 8v5M12 16.5v.01"></path></svg>
            <span>{t('tokenOnce')}</span>
          </Dialog.Description>
          {/if}
        </div>
        <div class="field">
          <span class="label">{t('messageTitle')}</span>
          <pre class="message-box">{message}</pre>
        </div>
        <div class="modal-actions start">
          <button type="button" class="btn primary tall" onclick={() => copy(message, 'message')}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"></rect><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"></path></svg>
            {copied === 'message' ? t('copiedShort') : t('copyMessage')}
          </button>
          <Dialog.Close class="btn tall">{t('done')}</Dialog.Close>
        </div>
        <p class="field-help">{viaGoogle ? t('privateChannelGoogle', issued.handle) : t('privateChannel', issued.handle)}</p>
      {/if}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<AlertDialog.Root bind:open={confirmOpen}>
  <AlertDialog.Portal>
    <AlertDialog.Overlay class="dialog-overlay" />
    <AlertDialog.Content class="modal small">
      {#if confirm}
        <div class="title-row">
          <span class="badge-icon danger" aria-hidden="true">
            {#if confirm.kind === 'disable'}
              <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M5.6 5.6l12.8 12.8"></path></svg>
            {:else if confirm.kind === 'delete'}
              <svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"></path></svg>
            {:else}
              <svg viewBox="0 0 24 24"><circle cx="8" cy="15" r="4"></circle><path d="m11 12 9-9M17 6l3 3M15 8l2 2"></path></svg>
            {/if}
          </span>
          <AlertDialog.Title class="modal-title">
            {t({ disable: 'disableTitle', delete: 'deleteTitle', regenerate: 'regenTitle' }[confirm.kind], confirm.user.handle)}
          </AlertDialog.Title>
        </div>
        <AlertDialog.Description class="modal-desc stack">
          {#if confirm.kind === 'disable'}
            {#each t('disableHelp') as line}<span>{line}</span>{/each}
          {:else if confirm.kind === 'delete'}
            <span>{t('deleteHelp')}</span>
          {:else}
            <span>{t('regenHelp')}</span>
          {/if}
        </AlertDialog.Description>
        {#if problem}<p class="form-error" role="alert">{problem}</p>{/if}
        <div class="modal-actions">
          <AlertDialog.Cancel class="btn tall">{t('cancel')}</AlertDialog.Cancel>
          <button type="button" class="btn danger solid tall" disabled={working} onclick={confirmed}>
            {t({ disable: 'disable', delete: 'deleteUser', regenerate: 'regenerate' }[confirm.kind])}
          </button>
        </div>
      {/if}
    </AlertDialog.Content>
  </AlertDialog.Portal>
</AlertDialog.Root>

<style>
  .reveal { align-self: flex-start; }
  .toast { position: fixed; left: 50%; bottom: 24px; transform: translateX(-50%); z-index: 60; margin: 0; padding: 10px 16px;
    border-radius: 999px; background: var(--ink); color: var(--bg); font-size: 13px; box-shadow: var(--shadow); }
  .notice svg, .act svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .act svg.dots { fill: currentColor; stroke: none; }
  .notice { margin: 0; display: flex; gap: 12px; align-items: flex-start; padding: 12px 16px; border-radius: 14px; background: var(--warn-soft);
    border: 1px solid var(--warn-line); color: var(--warn); font-size: 13px; }
  .notice svg { width: 18px; height: 18px; margin-top: 1px; flex: 0 0 auto; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip { display: inline-flex; align-items: center; gap: 7px; height: 32px; padding: 0 13px; border-radius: 999px; border: 1px solid var(--line);
    background: var(--surface); color: var(--ink-2); font-size: 13px; font-weight: 500; cursor: pointer; }
  .chip:hover { border-color: var(--line-strong); color: var(--ink); }
  .chip[aria-pressed="true"] { background: var(--ink); border-color: var(--ink); color: var(--bg); font-weight: 600; }
  .chip .n { font-size: 11px; opacity: .65; }
  .dot { width: 7px; height: 7px; border-radius: 50%; }
  .dot.active { background: var(--live); }
  .dot.pending { border: 2px solid var(--warn); box-sizing: border-box; }
  .dot.disabled { background: var(--muted); }
  .rows { list-style: none; margin: 0; padding: 0; border-radius: 18px; border: 1px solid var(--line); background: var(--surface);
    box-shadow: var(--shadow-sm); overflow: hidden; }
  .row { display: grid; grid-template-columns: minmax(220px, 2.2fr) minmax(90px, 1fr) minmax(120px, 1.2fr) minmax(130px, 1.2fr) minmax(110px, 1fr) 44px;
    align-items: center; gap: 12px; padding: 12px 20px; border-bottom: 1px solid var(--line); font-size: 13px; }
  .row:last-child { border-bottom: 0; }
  .row.header { padding-top: 12px; padding-bottom: 12px; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); }
  .person { display: flex; align-items: center; gap: 12px; min-width: 0; }
  .who { display: flex; flex-direction: column; min-width: 0; }
  .name { font: 600 14px/1.25 var(--sans); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .off .name { color: var(--muted); }
  .sub { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .handle { color: var(--ink-2); overflow: hidden; text-overflow: ellipsis; }
  .access { display: flex; flex-wrap: wrap; gap: 4px; }
  .tag { display: inline-flex; align-items: center; height: 22px; padding: 0 8px; border-radius: 6px; background: var(--k-note-bg); color: var(--k-note);
    font-size: 12px; font-weight: 500; }
  .state-pill { display: inline-flex; align-items: center; height: 22px; padding: 0 9px; border-radius: 999px; font-size: 12px; font-weight: 600; white-space: nowrap; }
  .state-pill.admin { color: var(--accent-ink); background: var(--accent-soft); }
  .state-pill.active { color: var(--k-progress); background: var(--k-progress-bg); }
  .state-pill.pending { color: var(--warn); background: var(--warn-soft); }
  .state-pill.disabled { color: var(--k-note); background: var(--k-note-bg); }
  .when { white-space: nowrap; }
  .act { display: flex; justify-content: flex-end; }
  .console { margin: 0; font-size: 12px; }
  .invite { display: flex; flex-direction: column; gap: 16px; }
  :global(.modal .label) { margin: 0; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); }
  :global(.modal-actions.start) { justify-content: flex-start; }
  :global(.modal-desc.stack) { display: flex; flex-direction: column; gap: 8px; }
  @media (max-width: 900px) {
    .row.header { display: none; }
    .row { grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: 'person act' 'meta meta'; gap: 8px 12px; padding: 12px 8px 12px 14px; }
    .person { grid-area: person; }
    .act { grid-area: act; }
    .handle, .access, .when { display: none; }
    .row > span:nth-child(4) { grid-area: meta; padding-left: 46px; }
    .act :global(.icon-btn) { width: 44px; height: 44px; border: 0; background: transparent; }
  }
</style>
