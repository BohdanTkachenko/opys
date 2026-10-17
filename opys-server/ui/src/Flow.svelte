<script>
  import { untrack } from 'svelte';
  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { corpora } from './lib/corpora.svelte.js';
  import { affects, affectsProject, events } from './lib/events.svelte.js';
  import { statusTone } from './lib/format.js';
  import { createResource } from './lib/resource.svelte.js';
  import { docPath, href } from './lib/router.svelte.js';
  import { buildWorkflowMap, fitWorkflowMap, layoutWorkflowMap } from './lib/workflow-map.js';

  let { key } = $props();
  const graph = createResource();
  const diagram = createResource();
  let selectedType = $state(null);
  let selected = $state(null);
  let zoom = $state(1);
  let viewportWidth = $state(620);
  let viewportHeight = $state(560);
  let inspectOpen = $state(false);
  let search = $state('');

  async function load(wanted) {
    const view = await api.flow(wanted);
    return { ...view, documents: await api.docs(view.cid) };
  }
  $effect(() => {
    const wanted = key;
    selectedType = null;
    selected = null;
    search = '';
    zoom = 1;
    inspectOpen = false;
    graph.run(() => load(wanted));
  });
  $effect(() => {
    const wanted = key;
    return events.subscribe((batch) => {
      if (affectsProject(batch, wanted) || !graph.data?.cid || affects(batch, graph.data.cid)) {
        graph.run(() => load(wanted), { quiet: true });
      }
    });
  });
  const view = $derived(graph.data);
  const group = $derived(corpora.groups.find((candidate) => candidate.key === key));
  const documents = $derived(view?.documents ?? []);
  const model = $derived(buildWorkflowMap(view, null, documents));
  const layoutSignature = $derived(JSON.stringify(model));
  const activeId = $derived(model.nodes.some((node) => node.id === selected) ? selected
    : null);
  const active = $derived(model.nodes.find((node) => node.id === activeId));
  const currentDocs = $derived(active ? documents.filter((doc) => doc.status === active.status) : []);
  const shownDocs = $derived(currentDocs.filter((doc) => `${doc.id} ${doc.title}`.toLowerCase().includes(search.toLowerCase()))
    .sort((a, b) => (a.priority ?? Infinity) - (b.priority ?? Infinity) || a.id.localeCompare(b.id)));
  const blockers = $derived(currentDocs.filter((doc) => doc.blocked).length);
  const nextItems = $derived(view?.nodes.find((node) => node.status === active?.status)?.items ?? []);
  const outgoing = $derived(model.edges.filter((edge) => edge.from === activeId));
  const stage = $derived(diagram.data);
  const fit = $derived(stage ? fitWorkflowMap(stage.width, stage.height, viewportWidth, viewportHeight) : 1);
  const scale = $derived(fit * zoom);

  $effect(() => {
    void layoutSignature;
    const wanted = untrack(() => model);
    diagram.run(() => layoutWorkflowMap(wanted), { quiet: untrack(() => diagram.data !== null) });
  });
  function choose(id) { selected = id; selectedType = null; search = ''; inspectOpen = true; }
  function highlightType(name) { selectedType = name; selected = null; inspectOpen = false; }
  function count(status) { return documents.filter((doc) => doc.status === status).length; }
  function label(node) { return node?.status || '(no status)'; }
  function connected(edge) { return edge.from === activeId || edge.to === activeId || (selectedType && edge.types.includes(selectedType)); }
  function measureViewport(element) {
    let frame;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => { viewportWidth = element.clientWidth; viewportHeight = element.clientHeight; });
    };
    const observer = new ResizeObserver(update);
    observer.observe(element);
    update();
    return { destroy() { observer.disconnect(); cancelAnimationFrame(frame); } };
  }
</script>

<header class="head topbar">
  <div class="page-title"><span class="map-icon"><Icon name="flow" size={20} /></span><div><h1>{group?.name ?? 'Lifecycle'} <span>lifecycle</span></h1><p>How work can move, and what is waiting at each stage.</p></div></div>
  {#if view}<span class="small muted">{view.documents.length} documents in this project</span>{/if}
</header>

{#if graph.error}
  <div class="notice bad"><p>{graph.error.message}</p><button class="btn small" onclick={() => graph.run(() => load(key))}>Retry</button></div>
{/if}
{#if !view && !graph.error}
  <p class="muted">Loading the lifecycle…</p>
{:else if view}
  <nav class="workflow-tabs" aria-label="Highlight document type">
    <button class:active={!selectedType} aria-pressed={!selectedType} onclick={() => highlightType(null)}>All types<span>{documents.length}</span></button>
    {#each view.types as type (type.name)}
      <button class:active={selectedType === type.name} aria-pressed={selectedType === type.name} onclick={() => highlightType(type.name)}>{type.name}<span>{view.documents.filter((doc) => doc.type === type.name).length}</span></button>
    {/each}
  </nav>

  {#if model.types.length}
  <div class="workspace" class:inspecting={inspectOpen}>
    <section class="map" aria-label="Complete project lifecycle graph">
      <div class="map-heading"><div><strong>Project graph</strong><span class="mode">{model.types.length} document types</span></div><span class="map-count">{model.nodes.length} statuses · {model.edges.length} transitions</span></div>
      {#if diagram.error}
        <div class="notice bad"><p>Could not lay out this workflow: {diagram.error.message}</p><button class="btn small" onclick={() => diagram.run(() => layoutWorkflowMap(model))}>Retry layout</button></div>
      {:else if !stage || diagram.loading}
        <p class="layout-loading" role="status">Arranging the workflow…</p>
      {:else}
      <div class="map-viewport" use:measureViewport>
        <div class="stage" style:width={`${stage.width * scale}px`} style:height={`${stage.height * scale}px`}>
          <svg class="connections" width={stage.width * scale} height={stage.height * scale} viewBox={`0 0 ${stage.width} ${stage.height}`} aria-hidden="true">
            <defs>
              <marker id="workflow-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto"><path d="M 1 1 L 7 4 L 1 7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></marker>
              <marker id="workflow-arrow-selected" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto"><path d="M 1 1 L 7 4 L 1 7" fill="none" stroke="var(--accent)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></marker>
            </defs>
            {#each stage.edges as edge (edge.id)}
              {#each edge.paths as path}
                <path class="connection" class:related={connected(edge)} class:unrestricted={edge.unrestricted} d={path} marker-end={connected(edge) ? 'url(#workflow-arrow-selected)' : 'url(#workflow-arrow)'}><title>{model.nodes.find((node) => node.id === edge.from)?.status} → {model.nodes.find((node) => node.id === edge.to)?.status}: {edge.types.join(', ')}</title></path>
              {/each}
            {/each}
          </svg>
          {#each stage.nodes as node (node.id)}
            {@const tone = statusTone(node.status)}
            <button class="state" class:typed={selectedType && node.types.includes(selectedType)} class:selected={activeId === node.id} class:terminal={node.terminal} class:unknown={node.declared === false}
              style:--tone={tone ?? 210} style:width={`${node.width}px`} style:height={`${node.height}px`}
              style:transform={`translate(${node.x * scale}px, ${node.y * scale}px) scale(${scale})`}
              title={`${label(node)} · ${node.types.join(', ')}`} aria-pressed={activeId === node.id} aria-label={`Inspect ${label(node)}: ${count(node.status)} documents`}
              onclick={() => choose(node.id)}>
                <span class="state-label"><span class="state-dot"></span><span class="mono">{label(node)}</span></span><span class="state-count">{count(node.status)}</span>
                <span class="state-caption">{node.types.join(' · ') || 'Undeclared status'}</span>
            </button>
          {/each}
        </div>
      </div>
      {/if}
      <div class="map-footer"><span>Every status and transition is shown. Select a type to highlight its workflow.</span><div class="zoom"><button aria-label="Zoom out" onclick={() => zoom = Math.max(0.2, zoom - 0.2)}>−</button><button onclick={() => zoom = 1} aria-label="Fit workflow">Fit</button><button aria-label="Zoom in" onclick={() => zoom = Math.min(6, zoom + 0.4)}>+</button></div></div>
      <div class="workflow-explanation"><Icon name="flow" size={15} /><p>Shared statuses combine documents from every type. Dashed arrows are unrestricted moves; solid arrows are configured transitions. Counts update live. Select a status to inspect its documents.</p></div>
    </section>

    {#if inspectOpen}
    <aside class="inspector" aria-label="Selected workflow status">
      {#if active}
        <div class="inspector-heading"><div class="inspector-top"><span class="inspector-kicker">Status across all types</span><button class="close-inspector" aria-label="Close inspector" onclick={() => { inspectOpen = false; selected = null; }}><Icon name="x" size={16} /></button></div><h2 class="mono">{label(active)}</h2><div class="status-totals"><strong>{currentDocs.length}</strong> {currentDocs.length === 1 ? 'document' : 'documents'}{#if blockers}<span class="blocked-count"><Icon name="lock" size={11} />{blockers} blocked</span>{/if}</div><p class="owner-counts">{#each active.types as name}<span>{name} {currentDocs.filter((doc) => doc.type === name).length}</span>{/each}</p></div>
        <div class="movement"><h3>Next moves</h3>{#if outgoing.length}<div class="next-states">{#each outgoing as edge}{@const target = model.nodes.find((node) => node.id === edge.to)}<button title={`Allowed for ${edge.types.join(', ')}${edge.closingTypes.length ? '; close for '+edge.closingTypes.join(', ') : ''}`} onclick={() => choose(target.id)}>{label(target)}{#if edge.closing}<span>close</span>{/if}</button>{/each}</div>{:else}<p>{active.terminal ? 'Final status. Closed work items leave the inventory.' : 'No outgoing transitions configured.'}</p>{/if}</div>
        {#if nextItems.length}
          <div class="next-picks"><h3>Ready next <span>opys next</span></h3>{#each nextItems as item}<a href={href(docPath(view.cid, item.id))}><span class="mono">{item.id}</span><strong>{item.title}</strong></a>{/each}</div>
        {/if}
        <div class="document-heading"><h3>All documents</h3><span>{currentDocs.length}</span></div>
        {#if currentDocs.length}
          <label class="document-search"><Icon name="search" size={13} /><input bind:value={search} placeholder="Filter documents…" aria-label="Filter documents at this status" /></label>
          <div class="document-list">{#each shownDocs as doc (doc.id)}<a href={href(docPath(view.cid, doc.id))}><span class="document-id mono">{doc.id}<span>{doc.type}{#if doc.blocked}<span class="blocked-label"><Icon name="lock" size={10} />blocked</span>{/if}</span></span><span>{doc.title || doc.id}</span></a>{:else}<p class="empty-inspector">No matching documents.</p>{/each}</div>
        {:else}<div class="empty-inspector"><Icon name={active.terminal ? 'check' : 'doc'} size={22} /><p>{active.terminal ? 'Completed items are removed when closed.' : 'No documents at this status.'}</p></div>{/if}
      {:else}<div class="empty-inspector">Select a status to inspect its documents.</div>{/if}
    </aside>
    {/if}
  </div>
  {:else}
    <div class="notice"><p>No document workflows are configured for this project.</p></div>
  {/if}
{/if}

<style>
  .page-title { display:flex; align-items:center; gap:0.8rem; }
  .page-title h1 { font-size:1.2rem; margin:0; font-weight:650; }
  .page-title h1 span { color:var(--muted); font-weight:400; margin-left:0.3rem; }
  .page-title p { margin:0.3rem 0 0; color:var(--muted); font-size:0.85rem; }
  .map-icon { color:var(--accent); display:grid; place-items:center; width:2.4rem; height:2.4rem; border:1px solid var(--border); border-radius:10px; background:var(--panel); }
  .workflow-tabs { display:flex; gap:1.3rem; overflow-x:auto; border-bottom:1px solid var(--border); margin-bottom:1.2rem; }
  .workflow-tabs button { display:flex; align-items:center; gap:0.5rem; padding:0.75rem 0.1rem; background:none; color:var(--muted); border:0; border-bottom:2px solid transparent; cursor:pointer; font:inherit; font-size:0.9rem; white-space:nowrap; }
  .workflow-tabs button.active { color:var(--fg); border-bottom-color:var(--accent); }
  .workflow-tabs button span { font-size:0.72rem; background:var(--raised); padding:0.05rem 0.35rem; border-radius:4px; font-variant-numeric:tabular-nums; }
  .workspace { display:grid; grid-template-columns:minmax(0,1fr); gap:1.1rem; align-items:start; }
  .workspace.inspecting { grid-template-columns:minmax(0,1fr) 310px; }
  .map { min-width:0; border:1px solid var(--border); border-radius:12px; background:var(--panel); overflow:hidden; }
  .map-heading { display:flex; align-items:center; justify-content:space-between; gap:0.6rem; padding:1rem 1.1rem; border-bottom:1px solid var(--border); }
  .map-heading strong { font-size:0.95rem; }
  .mode { color:var(--muted); font-size:0.72rem; margin-left:0.65rem; }
  .map-count { color:var(--muted); font-size:0.75rem; white-space:nowrap; }
  .map-viewport { overflow:auto; padding:20px; height:clamp(360px,calc(100dvh - 340px),850px); display:grid; align-items:center; background:radial-gradient(circle,color-mix(in srgb,var(--border) 60%,transparent) 0.6px,transparent 0.7px); background-size:20px 20px; }
  .stage { position:relative; margin:auto; flex:none; }
  .connections { position:absolute; inset:0; overflow:visible; color:var(--border-strong); pointer-events:none; }
  .connection { fill:none; stroke:var(--border-strong); stroke-width:1.4; stroke-linecap:round; stroke-linejoin:round; }
  .connection.unrestricted { stroke-dasharray:3 4; }
  .connection.related { stroke:var(--accent); stroke-width:1.8; }
  .state { position:absolute; top:0; left:0; transform-origin:top left; display:grid; grid-template-columns:minmax(0,1fr) auto; align-content:center; gap:0.4rem 0.5rem; text-align:left; padding:14px 15px; border:1px solid var(--border-strong); border-radius:10px; background:var(--panel); color:var(--fg); cursor:pointer; box-shadow:0 2px 3px color-mix(in srgb,var(--fg) 4%,transparent); font:inherit; }
  .state:hover { border-color:var(--accent); }
  .state.selected { border-color:var(--accent); box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 12%,transparent); }
  .state.typed { border-color:var(--accent); }
  .state:focus-visible { outline:2px solid var(--accent); outline-offset:5px; }
  .state-label { display:flex; gap:7px; align-items:center; font-size:13px; white-space:nowrap; }
  .state-label .mono { overflow:hidden; text-overflow:ellipsis; }
  .state-dot { width:6px; height:6px; border-radius:50%; background:hsl(var(--tone) 65% 48%); flex:none; }
  .state-count { font-size:23px; font-weight:500; line-height:1; font-variant-numeric:tabular-nums; grid-row:span 2; align-self:center; }
  .state-caption { font-size:10px; color:var(--muted); padding-left:13px; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .state.terminal { border-style:dashed; }
  .state.unknown { border-color:var(--bad); }
  .map-footer { display:flex; justify-content:space-between; gap:0.5rem; align-items:center; padding:0.75rem 1rem; border-top:1px solid var(--border); font-size:0.7rem; color:var(--muted); }
  .map-footer>span { display:flex; align-items:center; gap:0.5rem; }
  .zoom { display:flex; border:1px solid var(--border); border-radius:6px; overflow:hidden; }
  .zoom button { border:0; border-left:1px solid var(--border); background:var(--panel); color:var(--muted); padding:0.2rem 0.5rem; cursor:pointer; font:inherit; }
  .zoom button:first-child { border-left:0; }
  .workflow-explanation { display:flex; gap:0.6rem; align-items:start; padding:0.8rem 1rem; color:var(--muted); border-top:1px solid var(--border); background:color-mix(in srgb,var(--raised) 35%,var(--panel)); }
  .workflow-explanation p { margin:0; font-size:0.76rem; line-height:1.6; }
  .workflow-explanation :global(svg) { margin-top:3px; color:var(--accent); }
  .inspector { border:1px solid var(--border); border-radius:12px; background:var(--panel); min-height:400px; overflow:hidden; }
  .inspector-heading { padding:1.2rem; border-bottom:1px solid var(--border); }
  .inspector-top { display:flex; align-items:center; justify-content:space-between; }
  .close-inspector { display:grid; place-items:center; padding:0.25rem; border:0; background:none; color:var(--muted); cursor:pointer; }
  .owner-counts { display:flex; flex-wrap:wrap; gap:0.4rem 0.65rem; margin:0.7rem 0 0; color:var(--muted); font-size:0.7rem; }
  .inspector-kicker { color:var(--muted); font-size:0.75rem; }
  .inspector h2 { margin:0.5rem 0 0.7rem; font-size:1.35rem; font-weight:550; overflow-wrap:anywhere; }
  .status-totals { display:flex; align-items:center; gap:0.35rem; font-size:0.8rem; color:var(--muted); }
  .status-totals strong { color:var(--fg); font-size:1.15rem; font-weight:550; }
  .blocked-count { margin-left:auto; display:flex; gap:0.25rem; align-items:center; color:var(--warn); font-size:0.75rem; }
  .inspector h3 { font-size:0.75rem; font-weight:600; margin:0 0 0.55rem; }
  .movement { padding:1rem 1.2rem; border-bottom:1px solid var(--border); }
  .movement p { margin:0; font-size:0.8rem; color:var(--muted); line-height:1.5; }
  .next-states { display:flex; flex-wrap:wrap; gap:0.35rem; }
  .next-states button { padding:0.25rem 0.5rem; background:var(--raised); border:1px solid var(--border); border-radius:5px; font:inherit; font-size:0.73rem; color:var(--fg); cursor:pointer; }
  .next-states span { color:var(--muted); margin-left:0.4rem; }
  .next-picks { padding:1rem 1.2rem; border-bottom:1px solid var(--border); background:color-mix(in srgb,var(--accent) 3%,var(--panel)); }
  .next-picks h3 { display:flex; justify-content:space-between; }
  .next-picks h3 span { font-weight:400; color:var(--muted); font-size:0.68rem; }
  .next-picks a { display:block; color:var(--fg); margin-top:0.7rem; text-decoration:none; font-size:0.8rem; }
  .next-picks a>span { color:var(--accent); font-size:0.7rem; display:block; margin-bottom:0.2rem; }
  .next-picks a strong { font-weight:500; line-height:1.5; }
  .document-heading { display:flex; justify-content:space-between; align-items:center; padding:1rem 1.2rem 0.6rem; }
  .document-heading h3 { margin:0; }
  .document-heading>span { color:var(--muted); font-size:0.75rem; }
  .document-search { display:flex; gap:0.4rem; align-items:center; margin:0 1.2rem 0.7rem; padding:0.3rem 0.5rem; border:1px solid var(--border); border-radius:6px; color:var(--muted); }
  .document-search input { width:100%; min-width:0; padding:0; border:0; background:transparent; box-shadow:none; font-size:0.75rem; }
  .document-list { max-height:350px; overflow:auto; padding:0 0.5rem 0.5rem; }
  .document-list a { display:block; padding:0.7rem; border-radius:7px; color:var(--fg); font-size:0.8rem; line-height:1.45; text-decoration:none; }
  .document-list a:hover { background:var(--raised); }
  .document-id { display:flex; align-items:center; justify-content:space-between; color:var(--muted); font-size:0.68rem; margin-bottom:0.3rem; }
  .document-id>span { display:flex; align-items:center; gap:0.5rem; }
  .blocked-label { display:flex; gap:0.2rem; align-items:center; color:var(--warn); font-family:inherit; font-size:0.65rem; }
  .empty-inspector { padding:1rem 1.2rem 1.6rem; text-align:center; color:var(--muted); font-size:0.8rem; }
  .empty-inspector p { line-height:1.6; margin:0.6rem 0 0; }
  .layout-loading { min-height:530px; padding:2rem; color:var(--muted); font-size:0.85rem; }
  @media (max-width:1050px) { .workspace.inspecting { grid-template-columns:minmax(0,1fr) 280px; } }
  @media (max-width:850px) { .workspace.inspecting { grid-template-columns:1fr; } .inspector { min-height:0; } .page-title p { max-width:32ch; } }
</style>
