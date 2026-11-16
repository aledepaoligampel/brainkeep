<script>
  // Avatar of a user: Google photo when stored (validated host), else the initial.
  let { user, status = 'active', small = false } = $props();
  let broken = $state('');
  const initial = $derived([...(user.name || user.handle || '?')][0]);
</script>

<span class="person-avatar" class:off={status === 'disabled'} class:pending={status === 'pending'} class:small aria-hidden="true">
  {#if user.picture && broken !== user.picture}
    <img src={user.picture} alt="" referrerpolicy="no-referrer" onerror={() => (broken = user.picture)} />
  {:else}
    {initial}
  {/if}
</span>

<style>
  .small { width: 26px; height: 26px; font-size: 14px; box-shadow: 0 0 0 2px var(--surface); }
</style>
