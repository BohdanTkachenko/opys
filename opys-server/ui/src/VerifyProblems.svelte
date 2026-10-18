<script>
  import SourceExcerpt from './SourceExcerpt.svelte';
  import { api } from './lib/api.js';
  import { affects, events } from './lib/events.svelte.js';
  import { createResource } from './lib/resource.svelte.js';
  import { verificationFindings } from './lib/verification.js';
  import { docPath, href } from './lib/router.svelte.js';

  let { cid, id = null, path = null, onreveal = null } = $props();
  const report = createResource();
  let fixing = $state(false);
  let outcome = $state('');
  let fixError = $state('');
  let selectedSource = $state(null);
  const findings = $derived(verificationFindings(report.data, id, path));
  const problems = $derived(findings.map((finding) => finding.message));
  const canFix = $derived(problems.some(
    (problem) => (report.data?.fixable_problems ?? []).includes(problem),
  ));
  $effect(() => { void report.data; selectedSource = null; onreveal?.(null); });

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
    <section class:card-warning={id !== null} aria-label="Verification problems">
    {#if id}<p class="warning-title">Verification problems</p>{/if}
    <details>
      <summary>Issue details ({problems.length})</summary>
      <ul aria-label="Verification issues">
        {#each findings as finding, index (index)}
          <li>
            <span>{finding.message}</span>
            {#if finding.range && finding.excerpt}
              <div class="issue-actions">
                <button class="source-location" aria-expanded={selectedSource === index} onclick={() => selectedSource = selectedSource === index ? null : index} title="Show highlighted markdown source">{finding.path}:{finding.range.start.line}:{finding.range.start.column}</button>
                {#if id && onreveal && finding.body_range}<button class="btn small" onclick={() => onreveal(finding)}>Show in document</button>{:else if !id && finding.doc_id}<a href={href(docPath(cid, finding.doc_id))}>Open document</a>{/if}
              </div>
              {#if selectedSource === index}<SourceExcerpt {finding} />{/if}
            {/if}
          </li>
        {/each}
      </ul>
    </details>
    <button class="btn small" disabled={fixing || !canFix} onclick={fix}>
      {fixing ? 'Fixing…' : 'Fix'}
    </button>
    <p class="why">{canFix
      ? (id ? 'Apply available automatic fixes to this document.' : 'Apply available automatic fixes to this corpus.')
      : 'These issues require manual repair; no automatic fixes are available.'}</p>
    </section>
  {:else if !id || outcome}
    <p class="why">No verification issues in the latest report.</p>
  {/if}
{/if}
{#if fixError}<p role="alert">{fixError}</p>{/if}
{#if outcome}<p class="outcome" role="status">{outcome}</p>{/if}

<style>
  .card-warning {
    background: color-mix(in srgb, var(--bad) 8%, var(--panel));
    border: 1px solid color-mix(in srgb, var(--bad) 45%, var(--border));
    border-left: 3px solid var(--bad);
    border-radius: 8px;
    padding: 0.75rem 1rem;
    margin-bottom: 1rem;
  }
  .warning-title { color: var(--bad); font-weight: 600; margin: 0; }
  .card-warning .why { color: var(--muted); font-size: 0.9em; }
  details { margin-top: 0.75rem; }
  summary { cursor: pointer; }
  ul { max-height: 20rem; overflow-y: auto; padding-left: 1.5rem; }
  li { margin: 0.5rem 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  .issue-actions { display:flex; flex-wrap:wrap; align-items:center; gap:0.5rem; margin-top:0.35rem; }
  .source-location { padding:0; min-height:0; border:0; border-radius:2px; background:none; color:var(--accent); cursor:pointer; text-align:left; font:0.78rem var(--font-mono); overflow-wrap:anywhere; }
  .source-location:hover { text-decoration:underline; }
  .issue-actions a { font-size:0.78rem; }
  .outcome { white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
