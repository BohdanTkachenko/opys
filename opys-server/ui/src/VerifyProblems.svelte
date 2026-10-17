<script>
  import { api } from './lib/api.js';
  import { affects, events } from './lib/events.svelte.js';
  import { createResource } from './lib/resource.svelte.js';

  let { cid } = $props();
  const report = createResource();

  $effect(() => {
    const wanted = cid;
    report.run(() => api.verify(wanted));
    return events.subscribe((batch) => {
      if (affects(batch, wanted)) report.run(() => api.verify(wanted), { quiet: true });
    });
  });
</script>

{#if report.loading}
  <p class="why" role="status">Loading issue details…</p>
{/if}
{#if report.error}
  <p class="why" role="alert">Could not load issue details: {report.error.message}</p>
  <button class="btn small" onclick={() => report.run(() => api.verify(cid))}>Retry</button>
{/if}
{#if report.data}
  {#if report.data.load_error}
    <p class="why" role="alert">Could not refresh the inventory: {report.data.load_error}</p>
  {/if}
  {#if report.data.problems.length > 0}
    <details open>
      <summary>Issue details ({report.data.problems.length})</summary>
      <ul aria-label="Verification issues">
        {#each report.data.problems as problem}
          <li>{problem}</li>
        {/each}
      </ul>
    </details>
  {:else}
    <p class="why">No verification issues in the latest report.</p>
  {/if}
{/if}

<style>
  details { margin-top: 0.75rem; }
  summary { cursor: pointer; }
  ul { max-height: 20rem; overflow-y: auto; padding-left: 1.5rem; }
  li { margin: 0.5rem 0; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
