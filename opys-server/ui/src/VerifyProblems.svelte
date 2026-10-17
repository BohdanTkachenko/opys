<script>
  import { api } from './lib/api.js';
  import { affects, events } from './lib/events.svelte.js';
  import { createResource } from './lib/resource.svelte.js';

  let { cid, id = null } = $props();
  const report = createResource();
  let fixing = $state(false);
  let outcome = $state('');
  let fixError = $state('');
  const problems = $derived((report.data?.problems ?? []).filter(
    (problem) => !id || problem.startsWith(`${id}:`),
  ));

  async function fix() {
    fixing = true;
    outcome = '';
    fixError = '';
    try {
      const result = await api.action(cid, { action: 'verify-fix', ...(id ? { id } : {}) });
      outcome = [result.message, result.sync_skipped].filter(Boolean).join('\n');
      await report.run(() => api.verify(cid));
    } catch (error) {
      fixError = error.message;
    } finally {
      fixing = false;
    }
  }

  $effect(() => {
    const wanted = cid;
    report.run(() => api.verify(wanted));
    return events.subscribe((batch) => {
      if (affects(batch, wanted)) report.run(() => api.verify(wanted), { quiet: true });
    });
  });
</script>

{#if report.loading && !id}
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
  {#if problems.length > 0}
    <details open>
      <summary>Issue details ({problems.length})</summary>
      <ul aria-label="Verification issues">
        {#each problems as problem}
          <li>{problem}</li>
        {/each}
      </ul>
    </details>
    <button class="btn small" disabled={fixing} onclick={fix}>
      {fixing ? 'Fixing…' : 'Fix'}
    </button>
    <p class="why">{id ? 'Apply automatic fixes to this document.' : 'Apply automatic fixes to this corpus.'} Some issues may need manual repair.</p>
  {:else if !id || outcome}
    <p class="why">No verification issues in the latest report.</p>
  {/if}
{/if}
{#if fixError}<p role="alert">{fixError}</p>{/if}
{#if outcome}<p class="outcome" role="status">{outcome}</p>{/if}

<style>
  details { margin-top: 0.75rem; }
  summary { cursor: pointer; }
  ul { max-height: 20rem; overflow-y: auto; padding-left: 1.5rem; }
  li { margin: 0.5rem 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  .outcome { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
