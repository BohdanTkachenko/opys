<script>
  import { shortTime } from './lib/format.js';
  import { flowDocumentPath } from './lib/flow-progress.js';
  import { relativeSince } from './lib/queue.js';
  import { docPath, href, rolePath } from './lib/router.svelte.js';

  let { items, view, onselect } = $props();
  let now = $state(Date.now());
  $effect(() => {
    const timer = setInterval(() => now = Date.now(), 60_000);
    return () => clearInterval(timer);
  });

  function path(item) {
    return item.sources.map((source) => flowDocumentPath(source, item.id)).find(Boolean)
      ?? (item.primary && view.cid ? docPath(view.cid, item.id) : null);
  }
  function age(claim) {
    const elapsed = relativeSince(claim.since, now);
    return elapsed === 'just now' ? elapsed : elapsed ? `${elapsed} ago` : 'Age unknown';
  }
</script>

<section class="activity" aria-label="In progress">
  <header class="activity-title">
    <h2><span class="activity-dot" class:occupied={items.length > 0}></span>In progress</h2>
    <span class="activity-count" aria-live="polite">{items.length} claimed</span>
  </header>
  {#if items.length}
    <div class="activity-list">
      {#each items as item (JSON.stringify([item.id, item.status, item.ref ?? null, item.claim.holder]))}
        {@const owners = view.nodes.find((node) => node.status === item.status)?.owners ?? []}
        {@const documentPath = path(item)}
        <article class="activity-item">
          <div class="activity-meta">
            <button class="activity-stage mono" title={`Inspect ${item.status}`} onclick={() => onselect(`state-${encodeURIComponent(item.status)}`)}>{item.status}</button>
            <span title={item.claim.since ? `Last claim activity: ${shortTime(item.claim.since)}` : 'Claim age is unknown'}>{age(item.claim)}</span>
          </div>
          {#if documentPath}
            <a class="activity-document" href={href(documentPath)}><span class="mono">{item.id}</span><strong>{item.title || item.id}</strong></a>
          {:else}
            <div class="activity-document"><span class="mono">{item.id}</span><strong>{item.title || item.id}</strong></div>
          {/if}
          {#if owners.length}
            <div class="activity-roles"><span>Stage {owners.length === 1 ? 'role' : 'roles'}</span>{#each owners as owner}<a href={href(rolePath(owner))}>{owner}</a>{/each}</div>
          {/if}
          <p class="activity-holder">Claimed by <span class="mono">{item.claim.holder}</span></p>
        </article>
      {/each}
    </div>
  {:else}
    <p class="activity-empty">No visible cards are claimed.</p>
  {/if}
</section>

<style>
  .activity { display:flex; flex-direction:column; flex:none; min-height:0; max-height:46%; border-bottom:1px solid var(--border); }
  .activity-title { display:flex; flex:none; align-items:center; justify-content:space-between; gap:0.5rem; padding:0.9rem 0.95rem 0.7rem; }
  h2 { display:flex; align-items:center; gap:0.55rem; margin:0; font-size:0.9rem; font-weight:600; }
  .activity-dot { width:7px; height:7px; flex:none; border-radius:50%; background:var(--muted); }
  .activity-dot.occupied { background:var(--good); box-shadow:0 0 0 3px color-mix(in srgb,var(--good) 12%,transparent); }
  .activity-count { color:var(--muted); font-size:0.72rem; font-variant-numeric:tabular-nums; white-space:nowrap; }
  .activity-list { min-height:0; overflow-y:auto; overscroll-behavior:contain; padding:0 0.65rem 0.65rem; }
  .activity-item { border-left:2px solid var(--good); border-radius:0 5px 5px 0; padding:0.55rem 0.6rem; background:color-mix(in srgb,var(--good) 4%,var(--panel)); }
  .activity-item + .activity-item { margin-top:0.55rem; }
  .activity-meta { display:flex; align-items:center; justify-content:space-between; gap:0.5rem; margin-bottom:0.35rem; color:var(--muted); font-size:0.67rem; }
  .activity-stage { cursor:pointer; font-size:0.68rem; padding:0.1rem 0.3rem; color:color-mix(in srgb,var(--good) 80%,var(--fg)); background:var(--panel); border:1px solid color-mix(in srgb,var(--good) 35%,var(--border)); border-radius:4px; }
  .activity-document { display:block; color:var(--fg); text-decoration:none; line-height:1.4; overflow-wrap:anywhere; }
  .activity-document>span { display:block; color:var(--accent); font-size:0.68rem; margin-bottom:0.15rem; }
  .activity-document>strong { display:block; font-size:0.78rem; font-weight:500; }
  a.activity-document:hover strong { color:var(--accent); text-decoration:underline; }
  .activity-roles { display:flex; align-items:baseline; flex-wrap:wrap; gap:0.2rem 0.35rem; margin-top:0.4rem; font-size:0.65rem; overflow-wrap:anywhere; }
  .activity-roles>span { color:var(--muted); }
  .activity-roles a { color:var(--accent); text-decoration:none; }
  .activity-roles a:hover { text-decoration:underline; }
  .activity-holder { margin:0.3rem 0 0; color:var(--muted); font-size:0.62rem; overflow-wrap:anywhere; }
  .activity-empty { margin:0; padding:0 0.95rem 0.9rem; color:var(--muted); font-size:0.75rem; }
  a:focus-visible, button:focus-visible { outline:2px solid var(--accent); outline-offset:3px; }
  @media (max-width:850px) { .activity { max-height:none; } .activity-list { max-height:22rem; } }
</style>
