<script>
  // View 3: one document — its frontmatter, its rendered body, and the writes
  // the node will accept for it.
  //
  // This component owns the write path for the whole view: [`perform`] is passed
  // down to the action bar and also used by the chips (a tag's ×, a blocker's ×),
  // so every write reports its outcome in one place and only one can be in
  // flight at a time. That matters more than it looks: a write waits on the
  // inventory lock for up to ten seconds, and two overlapping ones would be two
  // ten-second waits with one visible answer.

  import DocActions from './DocActions.svelte';
  import { api } from './lib/api.js';
  import { affects, events } from './lib/events.svelte.js';
  import { docIdFromHref, fieldText, relationTitle, shortTime } from './lib/format.js';
  import { notice } from './lib/notice.svelte.js';
  import { createResource } from './lib/resource.svelte.js';
  import { boardPath, docPath, go, href } from './lib/router.svelte.js';

  let { cid, id } = $props();

  const doc = createResource();

  /**
   * Keys the view renders itself; the rest of `fields` is the custom ones.
   *
   * `updated` is in here because the header shows it: every document has one, so
   * leaving it out listed the same timestamp twice, in two formats. `created`
   * stays a custom row — nothing else renders it.
   */
  const RESERVED = new Set([
    'id',
    'status',
    'tags',
    'updated',
    'references',
    'blocked_by',
    'blocks',
  ]);

  /** The three relation maps, in the order they read as a sentence. */
  const RELATIONS = [
    { key: 'references', label: 'References' },
    { key: 'blocked_by', label: 'Blocked by' },
    { key: 'blocks', label: 'Blocks' },
  ];

  // The ids are parameters rather than closed-over props so that every read is
  // an explicit dependency of the effect that started it: the shell keys this
  // view, but a keyless render must still follow its props rather than keep
  // showing the document it opened with.
  function load(corpusId, docId, quiet = false) {
    return doc.run(() => api.doc(corpusId, docId), { quiet });
  }

  $effect(() => {
    load(cid, id);
  });

  $effect(() => {
    const [corpusId, docId] = [cid, id];
    return events.subscribe((batch) => {
      if (affects(batch, corpusId)) load(corpusId, docId, true);
    });
  });

  const d = $derived(doc.data);
  const custom = $derived(
    Object.entries(d?.fields ?? {}).filter(([key]) => !RESERVED.has(key)),
  );

  // The write path.
  /** A label while a write is in flight; also the "one at a time" latch. */
  let pending = $state(null);
  /** The last refusal, shown verbatim — the node's messages are written for people. */
  let problem = $state(null);
  /** Kept so a 503 can be retried unchanged; the node's answer is "try again". */
  let lastAttempt = $state(null);

  /**
   * Send one action, then reconcile the view with what happened.
   *
   * Returns the node's outcome, or `null` if it refused — the action bar uses
   * that to decide whether to clear its inputs.
   */
  async function perform(body, label) {
    if (pending) return null;
    pending = label;
    problem = null;
    notice.clear();
    try {
      const outcome = await api.action(cid, body);
      lastAttempt = null;
      // A skipped sync goes to the page-level notice rather than to local state:
      // `close` navigates away in this same tick, and that is precisely the
      // action whose skipped pass matters most — the deleted document's
      // references were not struck, so the corpus is left with dangling links.
      // A component's state would be unmounted before it could render.
      notice.show(outcome.sync_skipped);
      if (body.action === 'close') {
        // `close` deletes the file. There is no document left to show, so the
        // only honest destination is the board.
        go(boardPath(cid));
        return outcome;
      }
      // The node reloads the corpus before it answers, so this read already
      // reflects the write — no waiting for the event stream to catch up.
      await load(cid, id, true);
      return outcome;
    } catch (cause) {
      problem = cause;
      lastAttempt = { body, label };
      return null;
    } finally {
      pending = null;
    }
  }

  function retry() {
    if (lastAttempt) perform(lastAttempt.body, lastAttempt.label);
  }

  /**
   * Follow a linkified id inside the rendered body.
   *
   * The corpus rewrites bare `TASK-0001` mentions into links to the *file*
   * (`TASK-0001.md`), which is right for reading the markdown in an editor and
   * wrong here: following one would leave the SPA and land on the node's JSON
   * 404. So document links are routed, and external URLs are left alone.
   *
   * In-page anchors (`[Context](#context)`, a hand-written table of contents)
   * need catching too, and for a sharper reason: the fragment *is* this app's
   * router, so letting one through replaces the document with "No such view".
   * A body is arbitrary markdown and this idiom is common in it, so the link
   * is handled here — scrolled to if the target exists, swallowed if not.
   *
   * An action rather than an `onclick` attribute so the wrapper stays a plain
   * container: the interactive things are the anchors inside it, which already
   * have all the keyboard behaviour a link should have.
   */
  function markdownLinks(node) {
    function onclick(event) {
      if (event.defaultPrevented || event.button !== 0) return;
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const anchor = event.target.closest?.('a');
      if (!anchor || !node.contains(anchor)) return;
      const raw = anchor.getAttribute('href') ?? '';

      if (raw.startsWith('#')) {
        event.preventDefault();
        const fragment = raw.slice(1);
        if (fragment.length === 0) return;
        let target = null;
        try {
          target = node.querySelector(`#${CSS.escape(decodeURIComponent(fragment))}`);
        } catch {
          target = null;
        }
        target?.scrollIntoView({ block: 'start' });
        return;
      }

      const target = docIdFromHref(raw);
      if (!target) return;
      event.preventDefault();
      go(docPath(cid, target));
    }
    node.addEventListener('click', onclick);
    return {
      destroy() {
        node.removeEventListener('click', onclick);
      },
    };
  }
</script>

{#if doc.error && !doc.data}
  <p class="crumb small"><a href={href(boardPath(cid))}>← board</a></p>
  <div class="notice bad">
    <p>{doc.error.message}</p>
    <p class="why">
      {#if doc.error.status === 404}
        The corpus has no document with that id. It may have been closed — closing
        deletes the file and leaves a struck-through tombstone wherever it was
        referenced.
      {:else if doc.error.notLoaded}
        This corpus has never loaded, so there is nothing to read.
      {:else if doc.error.offline}
        The node is not answering.
      {:else}
        The node could not answer for this document.
      {/if}
    </p>
    <p><button class="btn small" onclick={() => load(cid, id)}>Try again</button></p>
  </div>
{:else if !doc.settled}
  <p class="muted">Loading {id}…</p>
{:else if d}
  <p class="crumb small">
    <a href={href(boardPath(cid))}>← board</a>
    <span class="muted mono">{d.path}</span>
  </p>

  <header class="head">
    <h1>{d.title || d.id}</h1>
    <div class="chips">
      <span class="chip mono">{d.id}</span>
      {#if d.type}<span class="chip">{d.type}</span>{/if}
      {#if d.status}<span class="chip status">{d.status}</span>{/if}
      <!-- The only place `updated` is rendered (it is in RESERVED), so it shows
           the whole timestamp rather than dropping the time of day. -->
      {#if d.updated}<span class="small muted">updated {shortTime(d.updated)}</span>{/if}
    </div>
  </header>

  {#if doc.error}
    <div class="notice warn"><p>Could not refresh: {doc.error.message}</p></div>
  {/if}

  <section class="meta panel">
    <div class="row">
      <span class="label small muted">Tags</span>
      <div class="chips">
        <!-- Unkeyed: nothing guarantees a document's tags are distinct, and a
             duplicate key throws out of the render — in production builds too,
             so one repeated word in frontmatter would blank this page. -->
        {#each d.tags as tag}
          <span class="chip">
            {tag}
            <button
              class="x"
              title={`remove the tag "${tag}"`}
              aria-label={`remove the tag ${tag}`}
              disabled={Boolean(pending)}
              onclick={() => perform({ action: 'tag', id: d.id, remove: tag }, 'removing a tag')}
            >
              ×
            </button>
          </span>
        {:else}
          <span class="small muted">none</span>
        {/each}
      </div>
    </div>

    {#each RELATIONS as relation (relation.key)}
      {@const entries = Object.entries(d[relation.key] ?? {})}
      {#if entries.length > 0}
        <div class="row">
          <span class="label small muted">{relation.label}</span>
          <div class="chips">
            {#each entries as [refId, title] (refId)}
              {@const shown = relationTitle(title)}
              {#if shown.struck}
                <!-- A tombstone: the document was closed and its file deleted,
                     so this is deliberately not a link to anywhere. -->
                <span class="chip struck" title="closed">
                  <span class="mono">{refId}</span>
                  {shown.text}
                </span>
              {:else}
                <a class="chip" href={href(docPath(cid, refId))}>
                  <span class="mono">{refId}</span>
                  {shown.text}
                </a>
              {/if}
              {#if relation.key === 'blocked_by' && !shown.struck}
                <button
                  class="btn small"
                  disabled={Boolean(pending)}
                  onclick={() =>
                    perform({ action: 'unblock', id: d.id, by: refId }, 'removing a blocker')}
                >
                  unblock
                </button>
              {/if}
            {/each}
          </div>
        </div>
      {/if}
    {/each}

    {#each custom as [key, value] (key)}
      <div class="row">
        <span class="label small muted">{key}</span>
        <span class="value">{fieldText(value)}</span>
      </div>
    {/each}
  </section>

  <DocActions doc={d} {pending} {perform} />

  {#if problem}
    <div class="notice bad">
      <!-- Verbatim. The node's refusals name the rule that was broken and often
           the exact command to satisfy it; a rewrite here would lose that. -->
      <p>{problem.message}</p>
      {#if problem.busy}
        <p class="why">
          Nothing was written. Another <code>opys</code> invocation was holding the
          inventory lock — the same request will work once it lets go.
        </p>
        <p><button class="btn small" onclick={retry}>Retry</button></p>
      {:else if problem.invalid}
        <p class="why">Nothing was written. The corpus refused this change.</p>
      {:else if problem.offline}
        <p class="why">The node did not answer, so it is not known whether anything was written.</p>
      {/if}
    </div>
  {/if}

  <!-- Rendered by the node with raw HTML left escaped, so this is the document's
       markdown and nothing else. The heading is hidden below: it is the same
       title already in the header, extracted from this very line. -->
  <article class="body" use:markdownLinks>
    {@html d.body_html}
  </article>
{/if}

<style>
  .head {
    display: grid;
    gap: 0.4rem;
    margin-bottom: 1rem;
  }

  h1 {
    margin: 0;
    font-size: 1.4rem;
    overflow-wrap: anywhere;
  }

  .status {
    border-color: var(--accent);
  }

  .meta {
    display: grid;
    gap: 0.45rem;
    padding: 0.7rem 0.8rem;
    margin-bottom: 1rem;
  }

  .row {
    display: grid;
    grid-template-columns: 7rem minmax(0, 1fr);
    gap: 0.5rem;
    align-items: baseline;
  }

  @media (max-width: 34rem) {
    .row {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  .label {
    text-transform: lowercase;
  }

  .value {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .x {
    border: none;
    background: none;
    padding: 0 0 0 0.1rem;
    margin: 0;
    line-height: 1;
    cursor: pointer;
    color: var(--muted);
  }

  .x:hover:not(:disabled) {
    color: var(--bad);
  }

  .x:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  /* The body is server-rendered HTML, so its styles have to reach into it — the
     one place in this app where a global selector is the right tool. Scoped to
     `.body`, so nothing here can leak into the UI's own markup. */
  .body {
    max-width: 46rem;
    overflow-wrap: anywhere;
  }

  .body :global(h1:first-child) {
    /* The document's own title, already in the header above. */
    display: none;
  }

  .body :global(h2) {
    font-size: 1.05rem;
    margin: 1.6rem 0 0.4rem;
    padding-bottom: 0.2rem;
    border-bottom: 1px solid var(--border);
  }

  .body :global(h3) {
    font-size: 0.98rem;
    margin: 1.2rem 0 0.3rem;
  }

  .body :global(p),
  .body :global(ul),
  .body :global(ol) {
    margin: 0.5rem 0;
  }

  .body :global(li) {
    margin: 0.15rem 0;
  }

  .body :global(pre) {
    background: var(--raised);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.6rem 0.7rem;
    overflow-x: auto;
  }

  .body :global(code) {
    background: var(--raised);
    border-radius: 3px;
    padding: 0.05em 0.25em;
  }

  .body :global(pre code) {
    background: none;
    padding: 0;
  }

  .body :global(blockquote) {
    margin: 0.6rem 0;
    padding-left: 0.8rem;
    border-left: 3px solid var(--border);
    color: var(--muted);
  }

  .body :global(table) {
    border-collapse: collapse;
    display: block;
    overflow-x: auto;
    max-width: 100%;
  }

  .body :global(th),
  .body :global(td) {
    border: 1px solid var(--border);
    padding: 0.25rem 0.5rem;
    text-align: left;
  }

  .body :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1.5rem 0;
  }

  .body :global(input[type='checkbox']) {
    /* Checklists are part of the document's content, not a control here: a
       write goes through an action, never through the rendered body. */
    pointer-events: none;
  }
</style>
