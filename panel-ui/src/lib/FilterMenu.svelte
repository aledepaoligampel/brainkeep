<script>
  // A filter pill backed by Bits Select (listbox, typeahead, keyboard). Empty value: any.
  import { Select } from 'bits-ui';
  import AgentIcon from './AgentIcon.svelte';
  import { t } from './state.svelte.js';

  let { label, value = '', options = [], onchange, agents = false } = $props();
  const ANY = '\u0000any';
  const current = $derived(options.find((o) => o.value === value));
  const items = $derived([{ value: ANY, label: t('any') }, ...options]);
</script>

<span class="pill-group" data-active={value ? '' : undefined}>
  <Select.Root type="single" value={value || ANY} {items} onValueChange={(v) => { const next = v === ANY ? '' : v; if (next !== value) onchange(next); }}>
    <Select.Trigger class="pill" aria-label={`${label}: ${current ? current.label : t('any')}`}>
      <span class="pill-label">{label}</span>
      {#if current}
        {#if agents}<AgentIcon agent={current.value} size={16} label={false} />{/if}
        <span class="pill-value">{current.label}</span>
      {/if}
      <svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
    </Select.Trigger>
    <Select.Portal>
      <Select.Content class="menu pick" align="start" sideOffset={8} collisionPadding={12}>
        <Select.Viewport class="pick-list">
          {#each items as option (option.value)}
            <Select.Item class="menu-item" value={option.value} label={option.label}>
              {#snippet children({ selected })}
                {#if agents && option.value !== ANY}<AgentIcon agent={option.value} size={18} label={false} />{/if}
                <span class="pick-text" class:muted={option.value === ANY}>{option.label}</span>
                {#if selected}<svg class="check" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5 10 17 19 7"></path></svg>{/if}
              {/snippet}
            </Select.Item>
          {/each}
        </Select.Viewport>
      </Select.Content>
    </Select.Portal>
  </Select.Root>
  {#if value}
    <button type="button" class="pill-x" aria-label={`${t('clear')} ${label}`} onclick={() => onchange('')}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"></path></svg>
    </button>
  {/if}
</span>
