<script>
  import FlowActivity from './FlowActivity.svelte';
  import FlowBlockers from './FlowBlockers.svelte';
  import WorkBadge from './WorkBadge.svelte';
  import { tick, untrack } from 'svelte';
  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { events } from './lib/events.svelte.js';
  import { affectsFlowProgress, buildFlowProgress, claimedFlowDocuments, filterFlowDocuments, flowDocumentPath, flowProgressCounts, flowStatusCounts } from './lib/flow-progress.js';
  import { statusTone } from './lib/format.js';
  import { createResource } from './lib/resource.svelte.js';
  import { docPath, href } from './lib/router.svelte.js';
  import { buildWorkflowMap, fitWorkflowMap, layoutWorkflowMap } from './lib/workflow-map.js';

  let { key } = $props();
  const graph = createResource();
  const diagram = createResource();
  let hiddenTypes = $state([]);
  let selected = $state(null);
  let zoom = $state(1);
  let viewportWidth = $state(620);
  let viewportHeight = $state(560);
  let search = $state('');

  async function load(wanted) {
    const view = await api.flow(wanted);
    return { ...view, documents: view.documents ?? await api.docs(view.cid) };
  }
  $effect(() => {
    const wanted = key;
    hiddenTypes = [];
    selected = null;
    search = '';
    zoom = 1;
    graph.run(() => load(wanted));
  });
  $effect(() => {
    const wanted = key;
    return events.subscribe((batch) => {
      const currentGroup = corpora.groups.find((candidate) => candidate.key === wanted);
      if (affectsFlowProgress(batch, wanted, currentGroup, graph.data)) {
        graph.run(() => load(wanted), { quiet: true });
      }
    });
  });
  const view = $derived(graph.data);
  const group = $derived(corpora.groups.find((candidate) => candidate.key === key));
  const progress = $derived(buildFlowProgress(view, view?.documents ?? []));
  const documents = $derived(filterFlowDocuments(progress.documents, { hiddenTypes }));
  const claims = $derived(claimedFlowDocuments(documents));
  const totals = $derived(flowProgressCounts(documents));
  const allTotals = $derived(flowProgressCounts(progress.documents));
  const allTypesVisible = $derived((view?.types ?? []).every((type) => !hiddenTypes.includes(type.name)));
  const model = $derived(buildWorkflowMap(view, null, documents, hiddenTypes));
  const layoutSignature = $derived(JSON.stringify(model));
  const activeId = $derived(model.nodes.some((node) => node.id === selected) ? selected
    : null);
  const active = $derived(model.nodes.find((node) => node.id === activeId));
  const currentDocs = $derived(active ? documents.filter((doc) => doc.status === active.status) : []);
  const currentTotals = $derived(flowStatusCounts(currentDocs));
  const shownDocs = $derived(filterFlowDocuments(currentDocs, { search })
    .sort((a, b) => Number(!!b.claim && b.unavailable == null) - Number(!!a.claim && a.unavailable == null) || Number(b.primary) - Number(a.primary) ||
      (a.priority ?? Infinity) - (b.priority ?? Infinity) || a.id.localeCompare(b.id)));
  const nextItems = $derived((view?.nodes.find((node) => node.status === active?.status)?.items ?? []).filter((item) => !hiddenTypes.includes(item.type)));
  const outgoing = $derived(model.edges.filter((edge) => edge.from === activeId));
  const stage = $derived(diagram.data);
  const fit = $derived(stage ? fitWorkflowMap(stage.width, stage.height, viewportWidth, viewportHeight) : 1);
  const scale = $derived(fit * zoom);

  $effect(() => {
    void layoutSignature;
    const wanted = untrack(() => model);
    diagram.run(() => layoutWorkflowMap(wanted));
  });
  $effect(() => {
    if (selected !== null && !model.nodes.some((node) => node.id === selected)) {
      selected = null;
      search = '';
    }
  });
  function choose(id) { selected = id; search = ''; }
  function toggleType(name) {
    hiddenTypes = hiddenTypes.includes(name) ? hiddenTypes.filter((type) => type !== name) : [...hiddenTypes, name];
    zoom = 1;
  }
  function showOnlyType(name) {
    hiddenTypes = (view?.types ?? []).map((type) => type.name).filter((type) => type !== name);
    zoom = 1;
  }
  function toggleAllTypes() {
    hiddenTypes = allTypesVisible ? (view?.types ?? []).map((type) => type.name) : [];
    zoom = 1;
  }
  function counts(status) { return flowStatusCounts(documents.filter((doc) => doc.status === status)); }
  function countDescription(total) { return `${total.unblocked} unblocked, ${total.blocked} blocked, ${total.unknown} with unknown blocker state, ${total.tickets} tickets`; }
  function claimedAt(status) { return claims.filter((doc) => doc.status === status).length; }
  function label(node) { return node?.status || '(no status)'; }
  function connected(edge) { return edge.from === activeId || edge.to === activeId; }
  function arrowMarker(edge, reverse = false) {
    const direction = reverse ? edge.reverse : edge.forward;
    if (!direction) return undefined;
    const highlighted = connected(edge);
    return highlighted ? 'url(#workflow-arrow-selected)' : 'url(#workflow-arrow)';
  }
  function connectionTitle(edge) {
    return [edge.forward, edge.reverse].filter(Boolean).map((direction) => {
      const from = model.nodes.find((node) => node.id === direction.from)?.status;
      const to = model.nodes.find((node) => node.id === direction.to)?.status;
      return `${from} → ${to}: ${direction.types.join(', ')} (${direction.unrestricted ? 'unrestricted' : 'configured'})${direction.closingTypes.length ? '; close for '+direction.closingTypes.join(', ') : ''}`;
    }).join('\n');
  }
  function canvasViewport(element) {
    let frame;
    let disposed = false;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => { viewportWidth = element.clientWidth; viewportHeight = element.clientHeight; });
    };
    const onwheel = async (event) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? element.clientHeight : 1);
      const next = Math.max(0.2, Math.min(6, zoom * Math.exp(-delta * 0.002)));
      const drawing = element.querySelector('.stage');
      if (next === zoom || !drawing) return;
      const before = drawing.getBoundingClientRect();
      const x = (event.clientX - before.left) / scale;
      const y = (event.clientY - before.top) / scale;
      zoom = next;
      await tick();
      if (disposed) return;
      // Keep the graph point under the cursor in place as the scrollable
      // drawing grows. Scroll bounds naturally center it again when it fits.
      const after = drawing.getBoundingClientRect();
      element.scrollLeft += after.left + x * scale - event.clientX;
      element.scrollTop += after.top + y * scale - event.clientY;
    };
    const observer = new ResizeObserver(update);
    observer.observe(element);
    element.addEventListener('wheel', onwheel, { passive: false });
    update();
    return { destroy() { disposed = true; observer.disconnect(); cancelAnimationFrame(frame); element.removeEventListener('wheel', onwheel); } };
  }
</script>

<div class="lifecycle">
<div class="workflow-main">
<header class="head topbar">
  <div class="header-summary">
    <div class="page-title"><span class="map-icon"><Icon name="flow" size={20} /></span><div><h1>{group?.name ?? 'Lifecycle'} <span>lifecycle</span></h1><p>Primary is truth. Branches show work in progress.</p></div></div>
    {#if view}<span class="document-total small muted">{totals.tickets}{#if !allTypesVisible} of {allTotals.tickets}{/if} tickets · {totals.primary} current{#if totals.progress} · {totals.progress} branch {totals.progress === 1 ? 'version' : 'versions'}{/if}</span>{/if}
  </div>
  {#if view}
  <nav class="workflow-tabs" aria-label="Visible document types">
    <button class:active={allTypesVisible} aria-pressed={allTypesVisible} onclick={toggleAllTypes}><span class="type-check">{#if allTypesVisible}<Icon name="check" size={11} />{/if}</span>All types<span>{allTotals.tickets}</span></button>
    {#each view.types as type (type.name)}
      {@const visible = !hiddenTypes.includes(type.name)}
      {@const only = visible && model.types.length === 1}
      <div class="type-control">
        <button class="type-toggle" class:active={visible} aria-pressed={visible} title={`${visible ? 'Hide' : 'Show'} ${type.name}`} onclick={() => toggleType(type.name)}><span class="type-check">{#if visible}<Icon name="check" size={11} />{/if}</span>{type.name}<span>{flowProgressCounts(progress.documents.filter((doc) => doc.type === type.name)).tickets}</span></button>
        <button class="type-only" class:active={only} aria-pressed={only} aria-label={`Show only ${type.name}`} title={`Show only ${type.name}`} onclick={() => showOnlyType(type.name)}>Only</button>
      </div>
    {/each}
  </nav>
  {/if}
</header>

{#if graph.error}
  <div class="notice bad"><p>{graph.error.message}</p><button class="btn small" onclick={() => graph.run(() => load(key))}>Retry</button></div>
{/if}
{#if !view && !graph.error}
  <p class="muted">Loading the lifecycle…</p>
{:else if view}

    {#if progress.notices.length}
      <div class="progress-notice notice"><ul>{#each progress.notices as message}<li>{message}</li>{/each}</ul></div>
    {/if}
    {#if progress.errors.length}
      <div class="progress-notice notice bad"><p>Some progress sources did not answer. Their documents are unknown, not absent.</p><ul>{#each progress.errors as source (source.key)}<li><strong>{source.label}:</strong> {source.error}</li>{/each}</ul></div>
    {/if}

    {#if model.types.length}
    <section class="map" aria-label="Project lifecycle graph for visible types">
      <div class="map-meta"><span>{model.types.length} document {model.types.length === 1 ? 'type' : 'types'}</span><span class="map-count">{model.nodes.length} statuses · {model.edges.length} transitions</span></div>
      {#if diagram.error}
        <div class="notice bad"><p>Could not lay out this workflow: {diagram.error.message}</p><button class="btn small" onclick={() => diagram.run(() => layoutWorkflowMap(model))}>Retry layout</button></div>
      {:else if !stage || diagram.loading}
        <p class="layout-loading" role="status">Arranging the workflow…</p>
      {:else}
      <div class="map-viewport" use:canvasViewport>
        <div class="stage" style:width={`${stage.width * scale}px`} style:height={`${stage.height * scale}px`}>
          <svg class="connections" width={stage.width * scale} height={stage.height * scale} viewBox={`0 0 ${stage.width} ${stage.height}`} aria-hidden="true">
            <defs>
              <marker id="workflow-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 1 1 L 7 4 L 1 7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></marker>
              <marker id="workflow-arrow-selected" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M 1 1 L 7 4 L 1 7" fill="none" stroke="var(--accent)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></marker>
            </defs>
            {#each stage.edges as edge (edge.id)}
              {#each edge.paths as path}
                <path class="connection" class:related={connected(edge)} class:unrestricted={edge.unrestricted} data-bidirectional={edge.bidirectional || undefined} d={path} marker-start={arrowMarker(edge, true)} marker-end={arrowMarker(edge)}><title>{connectionTitle(edge)}</title></path>
              {/each}
            {/each}
          </svg>
          {#each stage.nodes as node (node.id)}
            {@const tone = statusTone(node.status)}
            {@const total = counts(node.status)}
            <button class="state" class:empty={!total.tickets} class:all-blocked={total.tickets > 0 && total.blocked === total.tickets} class:selected={activeId === node.id} class:terminal={node.terminal} class:unknown={node.declared === false} class:claimed={claimedAt(node.status) > 0}
              style:--tone={tone ?? 210} style:width={`${node.width}px`} style:height={`${node.height}px`}
              style:transform={`translate(${node.x * scale}px, ${node.y * scale}px) scale(${scale})`}
              title={`${label(node)} · ${node.types.join(', ')} · ${countDescription(total)} · ${total.primary} current, ${total.progress} branch versions · ${claimedAt(node.status)} claimed`} aria-pressed={activeId === node.id} aria-label={`Inspect ${label(node)}: ${countDescription(total)}, ${total.primary} current, ${total.progress} branch versions, ${claimedAt(node.status)} claimed`}
              onclick={() => choose(node.id)}>
                <span class="state-label"><span class="state-dot"></span><span class="mono">{label(node)}</span></span><span class="state-count" class:empty={!total.tickets}>{total.tickets || 'Empty'}</span>
                <span class="state-caption">{node.types.join(' · ') || 'Undeclared status'}</span>
                {#if view.progress}<span class="state-progress">{total.primary} current · {total.progress} branch {total.progress === 1 ? 'version' : 'versions'}</span>{/if}
                {#if node.owners.length}<span class="state-owners">{#each node.owners as owner}<span title={owner}>{owner}</span>{/each}</span>{/if}
                <span class="state-blockers"><FlowBlockers counts={total} /></span>
                <span class="state-activity">{#if claimedAt(node.status)}<span class="claim-dot"></span>{claimedAt(node.status)} claimed{/if}</span>
            </button>
          {/each}
        </div>
      </div>
      {/if}
      <footer class="map-footer">
        <div class="map-legend" aria-label="Graph legend">
          <span title="Counts show total cards; badges identify blocked cards and unknown blocker states.">Total cards</span>
          <span title="Solid connections include configured transitions"><span class="legend-line" aria-hidden="true"></span>Configured</span>
          <span title="Dashed connections allow unrestricted transitions"><span class="legend-line dashed" aria-hidden="true"></span>Unrestricted</span>
          <span title="Arrowheads at both ends allow transitions in either direction"><svg class="legend-arrow" width="22" height="12" viewBox="0 0 22 12" aria-hidden="true"><path d="M2 6h18M6 2 2 6l4 4M16 2l4 4-4 4" /></svg>Two-way</span>
        </div>
        <div class="zoom" role="group" aria-label="Graph zoom"><button aria-label="Zoom out" onclick={() => zoom = Math.max(0.2, zoom - 0.2)}>−</button><button onclick={() => zoom = 1} aria-label="Fit workflow">Fit</button><button aria-label="Zoom in" onclick={() => zoom = Math.min(6, zoom + 0.4)}>+</button></div>
      </footer>
    </section>
    {:else if view.types.length}
      <div class="canvas-notice notice" role="status"><p>No types selected. Enable a type above to show its workflow.</p><button class="btn small" onclick={() => { hiddenTypes = []; zoom = 1; }}>Show all types</button></div>
    {:else}
      <div class="canvas-notice notice"><p>No document workflows are configured for this project.</p></div>
    {/if}
{/if}
</div>

    <aside class="inspector" aria-label="Workflow status details">
      {#if view}<FlowActivity items={claims} {view} onselect={choose} />{/if}
      <header class="inspector-title"><Icon name="flow" size={17} /><strong>Status details</strong></header>
      <div class="inspector-body">
      {#if active}
        {#if active.owners.length}<div class="status-owners"><span class="small muted">Waiting on</span>{#each active.owners as owner}<span class="chip">{owner}</span>{/each}</div>{/if}
        <div class="inspector-heading"><span class="inspector-kicker">Status across visible types</span><h2 class="mono">{label(active)}</h2><div class="status-totals"><strong title={countDescription(currentTotals)}>{currentTotals.tickets || 'Empty'}</strong>{#if currentTotals.tickets} {currentTotals.tickets === 1 ? 'card' : 'cards'}{/if}<FlowBlockers counts={currentTotals} /></div>{#if currentTotals.unknown}<p class="copy-counts">Blocker state unknown for {currentTotals.unknown} {currentTotals.unknown === 1 ? 'ticket' : 'tickets'}; {currentTotals.unblocked} known unblocked.</p>{/if}<p class="copy-counts">{currentTotals.primary} current · {currentTotals.progress} branch {currentTotals.progress === 1 ? 'version' : 'versions'}</p><p class="owner-counts">{#each active.types as name}<span>{name} {flowStatusCounts(currentDocs.filter((doc) => doc.type === name)).tickets}</span>{/each}</p></div>
        <div class="movement"><h3>Stage transitions</h3>{#if outgoing.length}<div class="next-states">{#each outgoing as edge}{@const target = model.nodes.find((node) => node.id === edge.to)}<button title={`Allowed for ${edge.types.join(', ')}${edge.closingTypes.length ? '; close for '+edge.closingTypes.join(', ') : ''}`} onclick={() => choose(target.id)}>{label(target)}{#if edge.closing}<span>close</span>{/if}</button>{/each}</div>{:else}<p>{active.terminal ? 'Final status. Work items leave the primary inventory when closed.' : 'No outgoing transitions configured.'}</p>{/if}</div>
        {#if nextItems.length}
          <div class="next-picks"><h3>Ready next <span>opys next</span></h3>{#each nextItems as item}<a href={href(docPath(view.cid, item.id))}><span class="mono">{item.id}</span><strong>{item.title}</strong><WorkBadge reference={item.ref} claim={item.claim} /></a>{/each}</div>
        {/if}
        <div class="document-heading"><h3>Documents and progress</h3><span>{currentDocs.length} {currentDocs.length === 1 ? 'version' : 'versions'}</span></div>
        {#if currentDocs.length}
          <label class="document-search"><Icon name="search" size={13} /><input bind:value={search} placeholder="Filter documents…" aria-label="Filter documents at this status" /></label>
          <div class="document-list">
            {#each shownDocs as doc (doc.key)}
              <article class="document-entry" class:progress-copy={!doc.primary} class:claimed={!!doc.claim && doc.unavailable == null}>
                <span class="document-id mono">{doc.id}<span>{doc.type ?? 'unknown type'}{#if doc.primary && doc.blocked}<span class="blocked-label"><Icon name="lock" size={10} />blocked</span>{/if}</span></span>
                {#if doc.primary}<a class="document-title" href={href(docPath(view.cid, doc.id))}>{doc.title || doc.id}</a>{:else}<span class="document-title">{doc.title || doc.id}</span>{/if}
                {#if doc.claim}<div class="document-sources"><WorkBadge claim={doc.claim} /></div>{/if}
                {#if doc.unavailable}<p class="progress-copy-note">Held: {doc.unavailable}</p>{/if}
                <div class="document-sources">
                  {#each doc.sources as source (source.key)}
                    {@const path = flowDocumentPath(source, doc.id)}
                    {#if path}<a class="source-chip" class:truth={source.primary} href={href(path)} title={source.work ? 'Declared work branch: current stage copy' : source.primary ? 'Primary working copy' : 'Open this worktree copy'}>{source.label}{#if source.work}<strong>work</strong>{:else if source.primary}<strong>main</strong>{/if}</a>
                    {:else}<span class="source-chip readonly" title={`Branch with no checkout; read-only${source.commit ? ` at ${source.commit}` : ''}`}>{source.label}<span>read-only</span></span>{/if}
                  {/each}
                </div>
                {#if !doc.primary}<p class="progress-copy-note">Branch progress · blockers unknown{#if doc.primaryCid} · <a href={href(docPath(doc.primaryCid, doc.id))}>primary copy</a>{/if}</p>{/if}
              </article>
            {:else}<p class="empty-inspector">No matching documents.</p>{/each}
          </div>
        {:else}<div class="empty-inspector"><Icon name={active.terminal ? 'check' : 'doc'} size={22} /><p>{active.terminal ? 'Completed items are removed when closed.' : 'No documents at this status.'}</p></div>{/if}
      {:else}<div class="empty-inspector"><p>Select a status on the canvas to see its documents and next moves.</p></div>{/if}
      </div>
    </aside>
</div>

<style>
  .lifecycle { display:grid; grid-template-columns:minmax(0,1fr) 310px; min-height:100dvh; }
  .workflow-main { display:flex; flex-direction:column; min-width:0; height:100dvh; }
  .head { display:flex; flex:none; flex-direction:column; gap:0.85rem; margin:0; padding:0.85rem 1.25rem; }
  .header-summary { display:flex; align-items:center; justify-content:space-between; flex-wrap:wrap; gap:0.6rem 1rem; }
  .document-total { font-variant-numeric:tabular-nums; }
  .page-title { display:flex; align-items:center; gap:0.8rem; }
  .page-title h1 { font-size:1.2rem; margin:0; font-weight:650; }
  .page-title h1 span { color:var(--muted); font-weight:400; margin-left:0.3rem; }
  .page-title p { margin:0.3rem 0 0; color:var(--muted); font-size:0.85rem; }
  .map-icon { color:var(--accent); display:grid; place-items:center; width:2.4rem; height:2.4rem; border:1px solid var(--border); border-radius:10px; background:var(--panel); }
  .workflow-tabs { display:flex; flex-wrap:wrap; gap:0.5rem; padding:0.15rem 2px; margin:0 -2px; }
  .workflow-tabs button { display:flex; align-items:center; gap:0.45rem; padding:0.45rem 0.65rem; background:var(--panel); color:var(--muted); border:1px solid var(--border); border-radius:7px; cursor:pointer; font:inherit; font-size:0.82rem; white-space:nowrap; }
  .workflow-tabs button.active { color:var(--fg); border-color:color-mix(in srgb,var(--accent) 45%,var(--border)); background:color-mix(in srgb,var(--accent) 5%,var(--panel)); }
  .type-control { display:flex; flex:none; }
  .workflow-tabs .type-toggle { border-radius:7px 0 0 7px; }
  .workflow-tabs .type-only { border-left:0; border-radius:0 7px 7px 0; padding-inline:0.45rem; font-size:0.7rem; }
  .workflow-tabs .type-only:hover:not(.active) { color:var(--accent); background:var(--raised); }
  .workflow-tabs .type-only.active { color:var(--accent-fg); background:var(--accent); border-color:var(--accent); }
  .workflow-tabs button:focus-visible { outline:2px solid var(--accent); outline-offset:2px; z-index:1; }
  .workflow-tabs button>span:last-child { font-size:0.7rem; background:var(--raised); padding:0.05rem 0.35rem; border-radius:4px; font-variant-numeric:tabular-nums; }
  .workflow-tabs .type-check { display:grid; place-items:center; width:13px; height:13px; padding:0; border:1px solid var(--muted); border-radius:3px; }
  .workflow-tabs .active .type-check { background:var(--accent); color:var(--accent-fg); border-color:var(--accent); }
  .map { display:flex; flex:1; flex-direction:column; min-width:0; min-height:0; background:radial-gradient(circle,var(--grid-dot) 0.8px,transparent 0.9px); background-size:20px 20px; }
  .map-meta { display:flex; flex:none; align-items:center; justify-content:space-between; flex-wrap:wrap; gap:0.4rem 0.75rem; padding:0.8rem 1.25rem; color:var(--muted); font-size:0.72rem; }
  .map-count { white-space:nowrap; font-variant-numeric:tabular-nums; }
  .map-viewport { overflow:auto; padding:20px; display:grid; align-items:center; flex:1; min-height:0; }
  .canvas-notice { margin:1.25rem; }
  .stage { position:relative; margin:auto; flex:none; }
  .connections { position:absolute; inset:0; overflow:visible; color:var(--border-strong); pointer-events:none; }
  .connection { fill:none; stroke:var(--border-strong); stroke-width:1.4; stroke-linecap:round; stroke-linejoin:round; }
  .connection.unrestricted { stroke-dasharray:3 4; }
  .connection.related { stroke:var(--accent); stroke-width:1.8; }
  .state { position:absolute; top:0; left:0; transform-origin:top left; display:grid; grid-template-columns:minmax(0,1fr) auto; align-content:center; gap:0.4rem 0.5rem; text-align:left; padding:14px 15px; border:1px solid var(--border-strong); border-radius:10px; background:var(--panel); color:var(--fg); cursor:pointer; box-shadow:0 2px 3px color-mix(in srgb,var(--fg) 4%,transparent); font:inherit; }
  .state.empty { background:var(--raised); }
  .state.all-blocked { background:color-mix(in srgb,var(--warn) 14%,var(--panel)); }
  .state:hover { border-color:var(--accent); }
  .state.selected { border-color:var(--accent); box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 12%,transparent); }
  .state:focus-visible { outline:2px solid var(--accent); outline-offset:5px; }
  .state-label { display:flex; gap:7px; align-items:center; font-size:13px; white-space:nowrap; }
  .state-label .mono { overflow:hidden; text-overflow:ellipsis; }
  .state-dot { width:6px; height:6px; border-radius:50%; background:hsl(var(--tone) 65% 48%); flex:none; }
  .state-count { font-size:23px; white-space:nowrap; font-weight:500; line-height:1; font-variant-numeric:tabular-nums; grid-row:span 2; align-self:center; }
  .state-count.empty { font-size:12px; color:var(--muted); }
  .state-blockers { grid-column:1 / -1; min-height:18px; }
  .state-caption { font-size:10px; color:var(--muted); padding-left:13px; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .state-progress { grid-column:1 / -1; color:var(--muted); font-size:9px; white-space:nowrap; }
  .state-owners { display:flex; flex-direction:column; align-items:flex-start; grid-column:1 / -1; gap:3px; overflow:hidden; }
  .state-owners span { border:1px solid var(--border); border-radius:4px; padding:1px 4px; max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:9px; color:var(--accent); }
  .state.claimed { border-color:color-mix(in srgb,var(--good) 65%,var(--border)); }
  .state-activity { display:flex; align-items:center; gap:5px; grid-column:1 / -1; min-height:14px; color:color-mix(in srgb,var(--good) 80%,var(--fg)); font-size:10px; font-weight:600; }
  .claim-dot { width:5px; height:5px; flex:none; border-radius:50%; background:currentColor; }
  .status-owners { display:flex; align-items:center; flex-wrap:wrap; gap:0.3rem; margin-bottom:0.7rem; }
  .state.terminal { border-style:dashed; }
  .state.unknown { border-color:var(--bad); }
  .map-footer { display:flex; flex:none; flex-wrap:wrap; gap:0.6rem 1rem; align-items:center; padding:0.7rem 1.25rem; font-size:0.7rem; color:var(--muted); }
  .map-legend { display:flex; flex-wrap:wrap; align-items:center; gap:0.55rem 0.9rem; }
  .map-legend>span { display:inline-flex; align-items:center; gap:0.4rem; white-space:nowrap; }
  .legend-line { width:20px; border-top:1.5px solid var(--border-strong); }
  .legend-line.dashed { border-top-style:dashed; }
  .legend-arrow { color:var(--border-strong); fill:none; stroke:currentColor; stroke-width:1.4; stroke-linecap:round; stroke-linejoin:round; }
  .zoom { display:flex; flex:none; margin-left:auto; border:1px solid var(--border); border-radius:6px; overflow:hidden; }
  .zoom button { border:0; border-left:1px solid var(--border); background:var(--panel); color:var(--muted); padding:0.2rem 0.5rem; cursor:pointer; font:inherit; }
  .zoom button:first-child { border-left:0; }
  .inspector { display:flex; flex-direction:column; position:sticky; top:0; align-self:start; height:100dvh; min-width:0; border-left:1px solid var(--border); background:var(--panel); }
  @supports (backdrop-filter:blur(1px)) { .inspector { background:var(--overlay); backdrop-filter:blur(14px); -webkit-backdrop-filter:blur(14px); } }
  .inspector-title { display:flex; flex:none; align-items:center; gap:0.6rem; padding:1rem 0.95rem; border-bottom:1px solid var(--border); font-size:0.9rem; }
  .inspector-title :global(svg) { color:var(--muted); }
  .inspector-body { flex:1; min-height:0; overflow-y:auto; }
  .inspector-heading { padding:1rem 0.95rem; border-bottom:1px solid var(--border); }
  .copy-counts { margin:0.5rem 0 0; font-size:0.73rem; color:var(--muted); }
  .owner-counts { display:flex; flex-wrap:wrap; gap:0.4rem 0.65rem; margin:0.7rem 0 0; color:var(--muted); font-size:0.7rem; }
  .inspector-kicker { color:var(--muted); font-size:0.75rem; }
  .inspector h2 { margin:0.5rem 0 0.7rem; font-size:1.35rem; font-weight:550; overflow-wrap:anywhere; }
  .status-totals { display:flex; flex-wrap:wrap; align-items:center; gap:0.35rem; font-size:0.8rem; color:var(--muted); }
  .status-totals strong { color:var(--fg); font-size:1.15rem; font-weight:550; }
  .inspector h3 { font-size:0.75rem; font-weight:600; margin:0 0 0.55rem; }
  .movement { padding:1rem 0.95rem; border-bottom:1px solid var(--border); }
  .movement p { margin:0; font-size:0.8rem; color:var(--muted); line-height:1.5; }
  .next-states { display:flex; flex-wrap:wrap; gap:0.35rem; }
  .next-states button { padding:0.25rem 0.5rem; background:var(--raised); border:1px solid var(--border); border-radius:5px; font:inherit; font-size:0.73rem; color:var(--fg); cursor:pointer; }
  .next-states span { color:var(--muted); margin-left:0.4rem; }
  .next-picks { padding:1rem 0.95rem; border-bottom:1px solid var(--border); background:color-mix(in srgb,var(--accent) 3%,var(--panel)); }
  .next-picks h3 { display:flex; justify-content:space-between; }
  .next-picks h3 span { font-weight:400; color:var(--muted); font-size:0.68rem; }
  .next-picks a { display:block; color:var(--fg); margin-top:0.7rem; text-decoration:none; font-size:0.8rem; }
  .next-picks a>span { color:var(--accent); font-size:0.7rem; display:block; margin-bottom:0.2rem; }
  .next-picks a strong { font-weight:500; line-height:1.5; }
  .document-heading { display:flex; justify-content:space-between; align-items:center; padding:1rem 0.95rem 0.6rem; }
  .document-heading h3 { margin:0; }
  .document-heading>span { color:var(--muted); font-size:0.75rem; }
  .document-search { display:flex; gap:0.4rem; align-items:center; margin:0 0.95rem 0.7rem; padding:0.3rem 0.5rem; border:1px solid var(--border); border-radius:6px; color:var(--muted); }
  .document-search input { width:100%; min-width:0; padding:0; border:0; background:transparent; box-shadow:none; font-size:0.75rem; }
  .document-list { padding:0 0.25rem 0.5rem; }
  .document-entry { margin:0.2rem 0; padding:0.7rem; border-radius:7px; font-size:0.8rem; line-height:1.45; }
  .document-entry.progress-copy { border-left:2px solid color-mix(in srgb,var(--accent-2) 45%,var(--border)); background:color-mix(in srgb,var(--accent-2) 3%,var(--panel)); }
  .document-entry.claimed { border-left:2px solid var(--good); background:color-mix(in srgb,var(--good) 4%,var(--panel)); }
  .document-title { display:block; color:var(--fg); text-decoration:none; overflow-wrap:anywhere; }
  a.document-title:hover { color:var(--accent); text-decoration:underline; }
  .document-sources { display:flex; flex-wrap:wrap; gap:0.3rem; margin-top:0.5rem; }
  .source-chip { display:inline-flex; align-items:center; gap:0.3rem; max-width:100%; padding:0.12rem 0.35rem; border:1px solid var(--border); border-radius:4px; color:var(--accent-2); font-size:0.65rem; text-decoration:none; overflow-wrap:anywhere; }
  a.source-chip:hover { background:var(--raised); border-color:var(--accent); }
  .source-chip.truth { color:var(--accent); }
  .source-chip strong { font-weight:500; }
  .source-chip.readonly>span { color:var(--muted); font-size:0.6rem; white-space:nowrap; }
  .progress-copy-note { margin:0.4rem 0 0; color:var(--muted); font-size:0.65rem; }
  .progress-copy-note a { color:inherit; }
  .progress-notice { flex:none; margin:0.7rem 1.25rem 0; max-height:9rem; overflow:auto; font-size:0.8rem; }
  .progress-notice ul { margin:0.2rem 0; padding-left:1rem; }
  .document-id { display:flex; align-items:center; justify-content:space-between; color:var(--muted); font-size:0.68rem; margin-bottom:0.3rem; }
  .document-id>span { display:flex; align-items:center; gap:0.5rem; }
  .blocked-label { display:flex; gap:0.2rem; align-items:center; color:var(--warn); font-family:inherit; font-size:0.65rem; }
  .empty-inspector { padding:1rem 0.95rem 1.6rem; color:var(--muted); font-size:0.8rem; }
  .empty-inspector p { line-height:1.6; margin:0.6rem 0 0; }
  .layout-loading { flex:1; padding:2rem; color:var(--muted); font-size:0.85rem; }
  @media (max-width:1050px) { .lifecycle { grid-template-columns:minmax(0,1fr) 280px; } }
  @media (max-width:850px) {
    .lifecycle { grid-template-columns:1fr; }
    .workflow-main { min-height:560px; }
    .head { padding:0.85rem 0.9rem; }
    .inspector { position:static; height:auto; min-height:20rem; border-left:0; border-top:1px solid var(--border); }
    .inspector-body { overflow-y:visible; }
    .page-title p { max-width:32ch; }
  }
</style>
