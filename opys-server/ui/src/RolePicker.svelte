<script>
  import { tick } from 'svelte';
  import Icon from './lib/Icon.svelte';
  import { groupRoles } from './lib/queue.js';

  let { entries = [], value = null, onchange } = $props();
  const id = $props.id();
  const groups = $derived(groupRoles(entries));
  const selected = $derived(entries.find((entry) => entry.name === value));
  let open = $state(false);
  let root = $state(null);
  let trigger = $state(null);
  let list = $state(null);
  let listHeight = $state(500);
  let typed = '';
  let typedAt = 0;

  function options() { return [...(list?.querySelectorAll('[role="option"]') ?? [])]; }
  function size() {
    if (open && trigger) listHeight = Math.max(100, window.innerHeight - trigger.getBoundingClientRect().bottom - 65);
  }
  async function show(last = false) {
    open = true;
    size();
    await tick();
    const rows = options();
    (rows.find((row) => row.dataset.name === value) ?? rows[last ? rows.length - 1 : 0])?.focus();
  }
  function close(restore = false) {
    open = false;
    if (restore) trigger?.focus();
  }
  function choose(name) {
    close(true);
    onchange(name);
  }
  function outside(event) {
    if (open && !root?.contains(event.target)) close();
  }
  function blur(event) {
    if (open && !root?.contains(event.relatedTarget)) close();
  }
  function keydown(event) {
    if (event.key === 'Escape' && open) {
      event.preventDefault(); event.stopPropagation(); close(true); return;
    }
    if (!open) {
      if (['ArrowDown', 'ArrowUp'].includes(event.key)) {
        event.preventDefault(); event.stopPropagation(); show(event.key === 'ArrowUp');
      }
      return;
    }
    const rows = options();
    const current = rows.indexOf(document.activeElement);
    let next;
    if (event.key === 'ArrowDown') next = (current + 1) % rows.length;
    else if (event.key === 'ArrowUp') next = (current - 1 + rows.length) % rows.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = rows.length - 1;
    else if (event.key.length === 1 && event.key !== ' ' && !event.ctrlKey && !event.metaKey && !event.altKey) {
      const now = Date.now();
      typed = now - typedAt < 700 ? typed + event.key.toLowerCase() : event.key.toLowerCase();
      typedAt = now;
      next = rows.findIndex((row) => row.dataset.name.toLowerCase().startsWith(typed));
    } else return;
    event.preventDefault(); event.stopPropagation();
    rows[next]?.focus();
  }
  function description(entry) {
    return entry.projects.filter((project) => project.description)
      .map((project) => entry.projects.length === 1 ? project.description : `${project.label}: ${project.description}`)
      .join('; ');
  }
</script>

<svelte:window onpointerdown={outside} onresize={size} onscroll={size} />

<div class="picker" bind:this={root} onfocusout={blur} role="group" aria-label="Role selection">
  <span class="caption small muted" id={`${id}-label`}>Waiting on</span>
  <button class="trigger" bind:this={trigger} type="button" aria-label="Choose a role"
    onkeydown={keydown} aria-haspopup="listbox" aria-expanded={open} aria-controls={`${id}-list`}
    onclick={() => open ? close() : show()}>
    <Icon name="queue" size={17} />
    <span class="current">{selected?.name ?? value ?? 'Choose a role…'}</span>
    {#if selected}<span class="count" class:zero={selected.total === 0} aria-label={`${selected.total} tickets`}>{selected.total}</span>{/if}
    <span class="chevron" class:expanded={open}><Icon name="chevron" size={15} /></span>
  </button>
  {#if open}
    <div class="menu">
      <div class="menu-head"><span>Roles</span><span>Tickets</span></div>
      <div class="list" style:max-height={`min(34rem, ${listHeight}px)`} bind:this={list} id={`${id}-list`} role="listbox" onkeydown={keydown} aria-labelledby={`${id}-label`} tabindex="-1">
        {#each groups as group (group.label)}
          <div class="kind-group" role="group" aria-label={group.label}>
            <div class="kind" aria-hidden="true"><span class="kind-dot" class:human={group.kind === 'human'} class:agent={group.kind === 'agent'}></span>{group.label}</div>
            {#each group.entries as entry (entry.name)}
              <button class="option" type="button" role="option" aria-selected={entry.name === value}
                data-name={entry.name} tabindex="-1" onclick={() => choose(entry.name)}>
                <span class="check">{#if entry.name === value}<Icon name="check" size={15} />{/if}</span>
                <span class="body"><span class="name">{entry.name}</span>{#if description(entry)}<span class="description">{description(entry)}</span>{/if}</span>
                <span class="count" class:zero={entry.total === 0} aria-label={`${entry.total} tickets`}>{entry.total}</span>
              </button>
            {/each}
          </div>
        {/each}
        {#if !groups.length}<p class="empty muted">No roles available.</p>{/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .picker { position:relative; width:21rem; max-width:100%; }
  .caption { display:block; margin-bottom:0.35rem; }
  .trigger { display:flex; align-items:center; gap:0.65rem; width:100%; min-height:2.8rem; padding:0.65rem 0.8rem; color:var(--fg); background:var(--panel); border:1px solid var(--border-strong); border-radius:8px; box-shadow:var(--shadow-1); text-align:left; cursor:pointer; }
  .trigger:hover, .trigger[aria-expanded="true"] { border-color:var(--accent); background:var(--raised); }
  .current { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-weight:550; }
  .chevron { display:flex; color:var(--muted); }
  .chevron.expanded { transform:rotate(180deg); }
  .count { flex:none; min-width:1.8rem; padding:0.12rem 0.4rem; border-radius:5px; text-align:center; font-family:var(--font-mono); font-size:0.8rem; font-variant-numeric:tabular-nums; background:color-mix(in srgb,var(--accent) 12%,var(--panel)); color:var(--accent); }
  .count.zero { color:var(--muted); background:var(--raised); }
  .menu { position:absolute; z-index:20; top:calc(100% + 0.45rem); right:0; width:32rem; max-width:calc(100vw - 3rem); border:1px solid var(--border-strong); border-radius:10px; background:var(--panel); box-shadow:var(--shadow-2); overflow:hidden; }
  .menu-head { display:flex; justify-content:space-between; padding:0.7rem 0.85rem; border-bottom:1px solid var(--border); color:var(--muted); font-size:0.75rem; }
  .list { max-height:min(34rem,60vh); overflow-y:auto; overscroll-behavior:contain; padding:0.35rem; }
  .kind-group + .kind-group { border-top:1px solid var(--border); margin-top:0.35rem; padding-top:0.35rem; }
  .kind { display:flex; align-items:center; gap:0.5rem; padding:0.55rem 0.55rem 0.45rem; font-size:0.75rem; font-weight:600; color:var(--muted); }
  .kind-dot { width:6px; height:6px; border-radius:50%; background:var(--muted); }
  .kind-dot.human { background:var(--accent); }
  .kind-dot.agent { background:var(--accent-2); }
  .option { display:flex; align-items:flex-start; gap:0.55rem; width:100%; border:0; border-radius:6px; padding:0.65rem 0.5rem; text-align:left; color:var(--fg); background:transparent; cursor:pointer; }
  .option:hover, .option:focus-visible { background:var(--raised); }
  .option[aria-selected="true"] { background:color-mix(in srgb,var(--accent) 7%,var(--panel)); }
  .check { width:1rem; min-height:1rem; flex:none; color:var(--accent); padding-top:0.1rem; }
  .body { display:grid; gap:0.22rem; flex:1; min-width:0; }
  .name { font-size:0.88rem; font-weight:550; overflow-wrap:anywhere; }
  .description { color:var(--muted); font-size:0.75rem; line-height:1.45; overflow-wrap:anywhere; }
  .trigger:focus-visible, .option:focus-visible { outline:2px solid var(--accent); outline-offset:-2px; }
  .empty { padding:0.8rem; font-size:0.85rem; }
  @media(max-width:46rem) { .picker { width:100%; } .menu { width:100%; max-width:100%; } }
</style>
