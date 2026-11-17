<script>
  // Shared page heading (title, optional count, description, actions) so every page
  // starts at the same place with the same type scale.
  let { id, title, count = '', description = '', actions } = $props();
</script>

<header class="page-head">
  <div class="intro">
    <div class="title">
      <h1 {id} class="serif">{title}</h1>
      {#if count}<span class="count">{count}</span>{/if}
    </div>
    {#if description}<p>{description}</p>{/if}
  </div>
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</header>

<style>
  .page-head { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 16px; }
  .intro { display: flex; flex-direction: column; gap: 6px; max-width: 640px; }
  .title { display: flex; align-items: baseline; gap: 12px; }
  h1 { margin: 0; font-size: 38px; line-height: 1.05; letter-spacing: -0.015em; }
  .count { color: var(--muted); }
  p { margin: 0; color: var(--ink-2); }
  .actions { display: flex; gap: 8px; }
  .actions :global(svg) { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  @media (max-width: 900px) {
    h1 { font-size: 30px; }
    .actions { width: 100%; }
    .actions :global(.btn) { width: 100%; }
  }
</style>
