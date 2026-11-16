<script>
  // Date range filter: quick presets plus a range calendar (Bits Popover + RangeCalendar).
  // Values are inclusive YYYY-MM-DD strings, as the API expects.
  import { Popover, RangeCalendar } from 'bits-ui';
  import { parseDate, today, getLocalTimeZone } from '@internationalized/date';
  import { app, t } from './state.svelte.js';

  let { from = '', to = '', onchange } = $props();
  let open = $state(false);
  let range = $state({ start: undefined, end: undefined });

  const safe = (s) => { try { return s ? parseDate(s) : undefined; } catch { return undefined; } };
  $effect(() => { if (open) range = { start: safe(from), end: safe(to) }; });

  const now = () => today(getLocalTimeZone());
  const PRESETS = [['today', 0], ['last7', 6], ['last30', 29], ['last90', 89]];
  function preset(days) {
    open = false;
    onchange(now().subtract({ days }).toString(), now().toString());
  }
  function picked(next) {
    if (next?.start && next?.end) {
      open = false;
      onchange(next.start.toString(), next.end.toString());
    }
  }
  const fmt = (s) => {
    const d = safe(s);
    return d ? d.toDate(getLocalTimeZone()).toLocaleDateString(app.lang, { day: 'numeric', month: 'short' }) : '';
  };
  const summary = $derived(
    from && to ? (from === to ? fmt(from) : `${fmt(from)} – ${fmt(to)}`) : from ? `≥ ${fmt(from)}` : to ? `≤ ${fmt(to)}` : '',
  );
  const active = $derived(PRESETS.find(([, d]) => to === now().toString() && from === now().subtract({ days: d }).toString()));
</script>

<span class="pill-group" data-active={summary ? '' : undefined}>
<Popover.Root bind:open>
  <Popover.Trigger class="pill" aria-label={`${t('date')}: ${summary || t('any')}`}>
    <span class="pill-label">{t('date')}</span>
    {#if summary}<span class="pill-value">{active ? t(active[0]) : summary}</span>{/if}
    <svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
  </Popover.Trigger>
  <Popover.Portal>
    <Popover.Content class="menu dates" align="start" sideOffset={8} collisionPadding={12}>
      <div class="presets" role="group" aria-label={t('date')}>
        {#each PRESETS as [key, days]}
          <button type="button" class="menu-item" aria-pressed={active?.[0] === key} onclick={() => preset(days)}>{t(key)}</button>
        {/each}
        <button type="button" class="menu-item muted" onclick={() => { open = false; onchange('', ''); }}>{t('anyDate')}</button>
      </div>
      <RangeCalendar.Root class="cal" bind:value={range} onValueChange={picked} locale={app.lang} weekdayFormat="short"
        fixedWeeks maxValue={now()}>
        {#snippet children({ months, weekdays })}
          <RangeCalendar.Header class="cal-head">
            <RangeCalendar.PrevButton class="btn icon small" aria-label={t('prevMonth')}>‹</RangeCalendar.PrevButton>
            <RangeCalendar.Heading class="cal-title" />
            <RangeCalendar.NextButton class="btn icon small" aria-label={t('nextMonth')}>›</RangeCalendar.NextButton>
          </RangeCalendar.Header>
          {#each months as month (month.value.toString())}
            <RangeCalendar.Grid class="cal-grid">
              <RangeCalendar.GridHead>
                <RangeCalendar.GridRow class="cal-row">
                  {#each weekdays as day}<RangeCalendar.HeadCell class="cal-wd">{day.slice(0, 2)}</RangeCalendar.HeadCell>{/each}
                </RangeCalendar.GridRow>
              </RangeCalendar.GridHead>
              <RangeCalendar.GridBody>
                {#each month.weeks as week}
                  <RangeCalendar.GridRow class="cal-row">
                    {#each week as date (date.toString())}
                      <RangeCalendar.Cell class="cal-cell" {date} month={month.value}>
                        <RangeCalendar.Day class="cal-day" />
                      </RangeCalendar.Cell>
                    {/each}
                  </RangeCalendar.GridRow>
                {/each}
              </RangeCalendar.GridBody>
            </RangeCalendar.Grid>
          {/each}
        {/snippet}
      </RangeCalendar.Root>
    </Popover.Content>
  </Popover.Portal>
</Popover.Root>
{#if summary}
    <button type="button" class="pill-x" aria-label={`${t('clear')} ${t('date')}`} onclick={() => onchange('', '')}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"></path></svg>
    </button>
  {/if}
</span>
