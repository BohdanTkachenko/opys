<script>
  import { relativeSince } from './lib/queue.js';
  import { shortTime } from './lib/format.js';
  let { reference = null, claim = null } = $props();
  let now = $state(Date.now());
  $effect(() => {
    if (!claim) return;
    const timer = setInterval(() => now = Date.now(), 60_000);
    return () => clearInterval(timer);
  });
</script>

{#if reference}<span class="chip small mono" title="Declared work branch">{reference}</span>{/if}
{#if claim}
  <span class="chip small" title={claim.since ? `Claim activity: ${shortTime(claim.since)}` : 'Claim age is unknown'}>
    claimed by {claim.holder} · {relativeSince(claim.since, now) ?? 'age unknown'}
  </span>
{/if}
