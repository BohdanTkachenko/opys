<script>
  import Icon from './lib/Icon.svelte';
  let { counts } = $props();
</script>

{#if counts.blocked || counts.unknown}
  <span class="blocker-summary">
    {#if counts.blocked && counts.unblocked && !counts.unknown}<span class="unblocked-count">{counts.unblocked} unblocked</span>{/if}
    {#if counts.blocked}<span class="blocked-badge"><Icon name="lock" size={10} />{counts.blockedLabel}</span>{/if}
    {#if counts.unknown}<span class="unknown-badge">{counts.unknown} unknown</span>{/if}
  </span>
{/if}

<style>
  .blocker-summary { display:inline-flex; align-items:center; flex-wrap:wrap; gap:4px 6px; font-size:10px; line-height:1.4; }
  .unblocked-count { color:var(--muted); white-space:nowrap; }
  .blocked-badge, .unknown-badge { display:inline-flex; align-items:center; gap:4px; padding:1px 5px; border:1px solid var(--border); border-radius:4px; white-space:nowrap; }
  .blocked-badge { color:color-mix(in srgb,var(--warn) 80%,var(--fg)); background:color-mix(in srgb,var(--warn) 8%,var(--panel)); border-color:color-mix(in srgb,var(--warn) 30%,var(--border)); }
  .unknown-badge { color:var(--muted); background:var(--raised); }
</style>
