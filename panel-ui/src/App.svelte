<script>
  import { onMount } from 'svelte';
  import Login from './Login.svelte';
  import Sidebar from './Sidebar.svelte';
  import MemoryList from './MemoryList.svelte';
  import MemoryDetail from './MemoryDetail.svelte';
  import Users from './Users.svelte';
  import Palette from './Palette.svelte';
  import { app, t, boot, applyTheme } from './lib/state.svelte.js';
  import { connect, disconnect } from './lib/live.js';

  let palette = $state(false);

  onMount(() => { applyTheme(); boot(); });
  $effect(() => { document.documentElement.lang = app.lang; });
  $effect(() => {
    if (app.session) connect();
    else disconnect();
  });

  function keydown(event) {
    if (!app.session) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); palette = true; }
    else if (event.key === '/' && !/^(INPUT|SELECT|TEXTAREA)$/.test(document.activeElement?.tagName || '')) {
      const search = document.getElementById('q');
      if (search) { event.preventDefault(); search.focus(); }
    }
  }
</script>

<svelte:window onkeydown={keydown} />

{#if !app.booted}
  <div class="boot" aria-busy="true"></div>
{:else if app.bootError}
  <div class="error boot-error" role="alert">{t('error')}</div>
{:else if !app.session}
  <Login />
{:else}
  <div class="shell" class:has-detail={Boolean(app.route.id)}>
    <aside class="nav-col"><Sidebar onsearch={() => (palette = true)} /></aside>
    <main class="main" class:wide={app.route.users}>
      {#if app.route.users}
        <Users />
      {:else}
      <div class="list-col"><MemoryList /></div>
      <div class="detail-col">
        {#if app.route.id}
          {#key app.route.project + '/' + app.route.id}
            <MemoryDetail project={app.route.project} id={app.route.id} />
          {/key}
        {:else}
          <div class="pick"><p class="serif">{t('pick')}</p></div>
        {/if}
      </div>
      {/if}
    </main>
  </div>
  <Palette bind:open={palette} />
{/if}

<style>
  .boot { min-height: 100vh; }
  .boot-error { margin: 32px; }
  .shell { min-height: 100vh; display: grid; grid-template-columns: 240px minmax(0, 1fr); }
  .nav-col { position: sticky; top: 0; height: 100vh; }
  .main { display: grid; grid-template-columns: minmax(360px, 1fr) minmax(0, 1.3fr); align-items: start; min-width: 0; }
  .main.wide { grid-template-columns: minmax(0, 1fr); }
  .list-col { min-width: 0; }
  .detail-col { position: sticky; top: 0; max-height: 100vh; overflow-y: auto; padding: 20px 20px 20px 0; display: flex; min-width: 0; }
  .pick { flex: 1; min-height: calc(100vh - 40px); display: flex; align-items: center; justify-content: center; border-radius: 20px;
    border: 1px dashed var(--line-strong); color: var(--muted); }
  .pick p { font-size: 22px; font-style: italic; }
  @media (max-width: 1180px) {
    .main { grid-template-columns: minmax(0, 1fr); }
    .detail-col { display: none; position: static; max-height: none; padding: 0; }
    .has-detail .list-col { display: none; }
    .has-detail .detail-col { display: flex; }
  }
  @media (max-width: 800px) {
    .shell { grid-template-columns: minmax(0, 1fr); grid-template-rows: auto 1fr; }
    .nav-col { position: static; height: auto; }
    /* Detail keeps the same top bar; only the project row folds away. */
    .has-detail .nav-col :global(.lists) { display: none; }
  }
</style>
