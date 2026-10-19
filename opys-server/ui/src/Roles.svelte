<script>
  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { events } from './lib/events.svelte.js';
  import { priorityChip, shortTime } from './lib/format.js';
  import { omni } from './lib/omni.svelte.js';
  import { affectsQueues, groupRoleQueues, relativeSince } from './lib/queue.js';
  import { createResource } from './lib/resource.svelte.js';
  import { roles } from './lib/roles.svelte.js';
  import { boardPath, docPath, go, href, rolePath } from './lib/router.svelte.js';

  let { role = null } = $props();
  const resource = createResource();
  const chosen = $derived(role ?? roles.remembered);
  const model = $derived(groupRoleQueues(resource.data, corpora.groups));
  const descriptions = $derived(roles.all.find((entry) => entry.name === chosen)?.projects ?? []);
  let listEl = $state(null);
  let now = $state(Date.now());
  let cursor = 0;

  $effect(() => {
    if (role) roles.remember(role);
  });
  $effect(() => {
    const wanted = chosen;
    if (wanted) resource.run(() => api.role(wanted));
  });
  $effect(() => {
    const wanted = chosen;
    return events.subscribe((batch) => {
      if (wanted && affectsQueues(batch, corpora.groups)) {
        resource.run(() => api.role(wanted), { quiet: true });
      }
    });
  });
  $effect(() => {
    const timer = setInterval(() => now = Date.now(), 60_000);
    return () => clearInterval(timer);
  });

  function choose(event) {
    const name = event.currentTarget.value;
    if (name) go(rolePath(name));
  }

  // Like the board, the cursor is real focus on a document link. Enter is
  // the link's native action; a refresh preserving the row preserves focus.
  function onkeydown(event) {
    if (omni.open || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
    if (event.target instanceof HTMLElement && event.target.closest('input, textarea, select, [contenteditable]')) return;
    if (!['j', 'k', 'ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const rows = [...(listEl?.querySelectorAll('.queue-row[href]') ?? [])]
      .filter((row) => row.getClientRects().length > 0);
    if (!rows.length) return;
    event.preventDefault();
    const held = rows.indexOf(document.activeElement);
    if (event.key === 'Home') cursor = 0;
    else if (event.key === 'End') cursor = rows.length - 1;
    else if (held >= 0) cursor = held + (['j', 'ArrowDown'].includes(event.key) ? 1 : -1);
    cursor = Math.max(0, Math.min(rows.length - 1, cursor));
    rows[cursor].focus({ preventScroll: true });
    rows[cursor].scrollIntoView({ block: 'nearest' });
  }
</script>

<svelte:window {onkeydown} />

{#snippet row(item, cid, attention = false)}
  {@const priority = priorityChip(item.priority, item.effective_priority)}
  {@const since = relativeSince(item.since, now)}
  <a class="queue-row" href={cid ? href(docPath(cid, item.id)) : undefined}>
    <span class="row-id mono">{item.id}</span>
    <span class="row-body">
      <span class="row-title">{item.title || item.id}</span>
      {#if attention}
        <span class="row-status small muted">{item.type ?? ''} · {item.status}</span>
        <span class="reasons">
          {#each item.reasons as reason}<span><strong>{reason.kind}:</strong> {reason.message}</span>{/each}
        </span>
      {/if}
    </span>
    <span class="row-marks">
      {#if since}<span class="small muted since" title={`At this status since ${shortTime(item.since)}`}>{since}</span>{/if}
      {#if priority}<span class="chip small" title={priority.title}>p{priority.rank}{#if priority.note}<span class="muted">{priority.note}</span>{/if}</span>{/if}
      {#each item.observations ?? [] as observation}<span class="chip observation" title={observation.message ?? observation.kind}><Icon name="clock" size={12} />{observation.kind}</span>{/each}
      {#if item.owner_override}<span class="chip override" title="Owner overridden for this step">override</span>{/if}
    </span>
  </a>
{/snippet}

<div class="roles-page">
  <header class="head topbar">
    <div><h1>Role queues</h1><p class="muted">Everything waiting on a role, across projects.</p></div>
    <label class="role-picker">
      <span class="small muted">Waiting on</span>
      <select value={chosen ?? ''} onchange={choose} aria-label="Choose a role">
        <option value="" disabled>Choose a role…</option>
        {#if chosen && !roles.all.some((entry) => entry.name === chosen)}<option value={chosen}>{chosen}</option>{/if}
        {#each roles.all as entry (entry.name)}
          <option value={entry.name}>{entry.name} ({entry.total}){entry.projects.some((project) => project.description) ? ` — ${entry.projects.filter((project) => project.description).map((project) => `${project.label}: ${project.description}`).join('; ')}` : ''}</option>
        {/each}
      </select>
    </label>
  </header>

  {#if roles.error}
    <div class="notice bad"><p>Could not load roles: {roles.error.message}</p><button class="btn small" onclick={() => roles.reload()}>Try again</button></div>
  {/if}
  {#if roles.errors.length}
    <div class="notice warn"><p>Some projects could not be asked.</p>{#each roles.errors as project (project.key)}<p><strong>{project.label}:</strong> {project.error}</p>{/each}</div>
  {/if}

  {#if !chosen}
    {#if !roles.settled}<p class="muted">Loading roles…</p>
    {:else if !roles.all.length && !roles.error}<p class="muted">No roles are declared by the projects this node serves.</p>
    {:else}<p class="muted">Choose a role to see its queues.</p>{/if}
  {:else}
    {#if descriptions.some((project) => project.description)}
      <div class="descriptions small muted">{#each descriptions.filter((project) => project.description) as project (project.key)}<p><strong>{project.label}:</strong> {project.description}</p>{/each}</div>
    {/if}
    {#if resource.error}
      <div class="notice bad"><p>{resource.data ? 'Could not refresh: ' : ''}{resource.error.message}</p><button class="btn small" onclick={() => resource.run(() => api.role(chosen))}>Try again</button></div>
    {/if}
    {#if !resource.data && !resource.error}<p class="muted">Loading {chosen}'s queues…</p>
    {:else if resource.data}
      <div bind:this={listEl} class="queues">
        {#if model.attention.length}
          <section class="attention" aria-label="Attention">
            <h2><Icon name="alert" size={18} /> Attention</h2>
            {#each model.attention as project (project.key)}
              <div class="attention-project">
                <h3>{#if project.cid}<a href={href(boardPath(project.cid))}>{project.label}</a>{:else}{project.label}{/if}</h3>
                {#each project.items as item (item.id)}{@render row(item, project.cid, true)}{/each}
                {#each project.problems as problem}<p class="project-problem">{problem}</p>{/each}
              </div>
            {/each}
          </section>
        {/if}

        {#each model.projects as project (project.key)}
          <section class="project" aria-label={project.label}>
            <h2>{#if project.cid}<a href={href(boardPath(project.cid))}>{project.label}</a>{:else}{project.label}{/if}</h2>
            {#if 'error' in project}<div class="notice bad"><p>{project.error}</p></div>
            {:else}
              {#each project.groups as group (`${group.type}/${group.status}`)}
                <section class="queue-group">
                  <header class="group-head">
                    <h3><span>{group.type}</span> <span class="mono">{group.status}</span></h3>
                    <span class="owners">{#each group.owners as owner}<a class="chip" href={href(rolePath(owner))}>{owner}</a>{/each}</span>
                    <span class="group-total small muted">{group.total} waiting · {group.blocked.length} blocked</span>
                  </header>
                  {#each group.items as item (item.id)}{@render row(item, project.cid)}{/each}
                  {#if group.total > group.items.length}<p class="more small muted">Showing {group.items.length} of {group.total} waiting.</p>{/if}
                  {#if group.blocked.length}
                    <details class="blocked"><summary>{group.blocked.length} blocked</summary>
                      {#each group.blocked as item (item.id)}<div>{@render row(item, project.cid)}<p class="blocked-by small muted">Blocked by {item.blocked_by.join(', ')}</p></div>{/each}
                    </details>
                  {/if}
                  {#if group.orphaned.length}
                    <details class="orphaned"><summary>{group.orphaned.length} orphaned</summary>
                      {#each group.orphaned as item (item.id)}<div>{@render row(item, project.cid)}<p class="blocked-by small muted">{item.reason}</p></div>{/each}
                    </details>
                  {/if}
                </section>
              {/each}
            {/if}
          </section>
        {/each}
        {#if model.empty}<p class="empty">Nothing is waiting on <code>{chosen}</code>.</p>{/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .roles-page { max-width:70rem; margin-inline:auto; }
  .head { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:1rem; }
  h1 { margin:0; font-size:1.4rem; }
  .head p { margin:0.3rem 0 0; }
  .role-picker { display:grid; gap:0.3rem; min-width:15rem; max-width:100%; }
  .role-picker select { max-width:32rem; width:100%; }
  .descriptions { margin:0 0 1.4rem; }
  .descriptions p { margin:0.25rem 0; }
  .queues { display:grid; gap:1.8rem; }
  h2 { display:flex; align-items:center; gap:0.5rem; margin:0 0 0.7rem; font-size:1.15rem; }
  h2 a, h3 a { color:inherit; text-decoration:none; }
  h2 a:hover, h3 a:hover { color:var(--accent); }
  .attention { border-left:3px solid var(--warn); padding-left:1rem; }
  .attention h2 { color:var(--warn); }
  .attention-project + .attention-project { margin-top:1rem; }
  .attention-project h3 { margin:0.5rem 0; font-size:0.9rem; }
  .project-problem { padding:0.6rem 0.7rem; color:var(--bad); overflow-wrap:anywhere; }
  .queue-group { margin-bottom:1rem; border:1px solid var(--border); border-radius:7px; overflow:hidden; }
  .group-head { display:flex; align-items:center; flex-wrap:wrap; gap:0.5rem 0.8rem; padding:0.7rem 0.8rem; background:var(--panel); border-bottom:1px solid var(--border); }
  .group-head h3 { display:flex; flex-wrap:wrap; gap:0.6rem; margin:0; font-size:0.85rem; font-weight:600; }
  .group-head h3>span:first-child { color:var(--muted); }
  .owners { display:flex; flex-wrap:wrap; gap:0.3rem; }
  .owners a { text-decoration:none; }
  .group-total { margin-left:auto; white-space:nowrap; font-variant-numeric:tabular-nums; }
  .queue-row { display:grid; grid-template-columns:7rem minmax(0,1fr) auto; gap:0.4rem 0.7rem; align-items:baseline; padding:0.7rem 0.8rem; color:inherit; text-decoration:none; border-bottom:1px solid var(--border); }
  .queue-row:last-child { border-bottom:0; }
  .queue-row:hover { background:var(--raised); }
  .queue-row:focus-visible { outline:2px solid var(--accent); outline-offset:-2px; background:var(--raised); }
  .row-id { color:var(--accent); font-size:0.78rem; overflow-wrap:anywhere; }
  .row-body { display:grid; gap:0.25rem; min-width:0; }
  .row-title { font-size:0.9rem; overflow-wrap:anywhere; }
  .row-marks { display:flex; justify-content:flex-end; align-items:center; flex-wrap:wrap; gap:0.35rem; }
  .since { white-space:nowrap; font-variant-numeric:tabular-nums; }
  .observation { color:var(--warn); font-size:0.7rem; }
  .override { color:var(--accent-2); font-size:0.7rem; }
  .reasons { display:grid; gap:0.3rem; color:var(--muted); font-size:0.8rem; overflow-wrap:anywhere; }
  .reasons strong { color:var(--warn); font-weight:500; }
  details { border-top:1px solid var(--border); }
  summary { padding:0.55rem 0.8rem; color:var(--muted); font-size:0.8rem; cursor:pointer; }
  summary:focus-visible { outline:2px solid var(--accent); outline-offset:-2px; }
  .blocked-by { margin:0; padding:0 0.8rem 0.65rem; overflow-wrap:anywhere; }
  .more { margin:0; padding:0.6rem 0.8rem; }
  .empty { margin:1rem 0; color:var(--muted); }
  @media (max-width:46rem) {
    .head { align-items:stretch; }
    .role-picker { width:100%; min-width:0; }
    .queue-row { grid-template-columns:6.5rem minmax(0,1fr); gap:0.4rem; }
    .row-marks { grid-column:2; justify-content:flex-start; }
    .group-total { width:100%; margin-left:0; }
    .attention { padding-left:0.65rem; }
  }
</style>
