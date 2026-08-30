<script>
  // View 3: one document — its frontmatter, its rendered body, and the writes
  // the node will accept for it.
  //
  // Everything about the document is edited where it is shown. The frontmatter
  // panel is one card of facts, each editable in place — status is a menu,
  // tags and blockers grow from an inline box on their own row, custom fields
  // open on click — and the body is click-to-edit prose. There is no separate
  // action bar: a control that lives apart from the fact it changes has to
  // explain which fact that is, and the explanation is the first thing a
  // reader has to scroll past to reach the document.
  //
  // This component owns the write path for the whole view: [`perform`] sends
  // every action, so every write reports its outcome in one place and only one
  // can be in flight at a time. That matters more than it looks: a write waits
  // on the inventory lock for up to ten seconds, and two overlapping ones would
  // be two ten-second waits with one visible answer.
  //
  // What can be set is driven by the payload rather than by knowledge of the
  // project's config. The status menu is `allowed_statuses` — the type's
  // statuses minus its terminal ones, computed by the node — and close is
  // gated on `closable`, because a type with no terminal status has nothing to
  // close *to*. A UI that derived either from `opys.toml` would be a second
  // interpretation of the config, wrong the first time a type was edited.

  import { tick } from 'svelte';
  import Icon from './lib/Icon.svelte';
  import { api } from './lib/api.js';
  import { affects, events } from './lib/events.svelte.js';
  import {
    docIdFromHref,
    fieldText,
    isTimestamp,
    relationTitle,
    relativeTime,
    shortTime,
    splitCsv,
    statusTone,
    typeTone,
  } from './lib/format.js';
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

  /**
   * The three relation maps, in the order they read as a sentence. Only
   * `blocked_by` is written from here (`block`/`unblock`, both sides at once):
   * `references` is reconciled by the engine from the prose, and `blocks` is
   * the other side of someone else's `blocked_by`.
   */
  const RELATIONS = [
    { key: 'references', label: 'References', editable: false },
    { key: 'blocked_by', label: 'Blocked by', editable: true },
    { key: 'blocks', label: 'Blocks', editable: false },
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

  /** A minute clock, so "updated 5m ago" in the header keeps being true. */
  let now = $state(Date.now());

  $effect(() => {
    const timer = setInterval(() => {
      now = Date.now();
    }, 60_000);
    return () => clearInterval(timer);
  });

  const d = $derived(doc.data);
  const custom = $derived(
    Object.entries(d?.fields ?? {}).filter(([key]) => !RESERVED.has(key)),
  );
  /** The type's declared field specs, keyed by name — the editor's affordances. */
  const declared = $derived(new Map((d?.declared_fields ?? []).map((f) => [f.name, f])));
  /** Declared fields this document has not set yet, offered by the add row. */
  const unsetDeclared = $derived(
    (d?.declared_fields ?? []).filter((f) => !(f.name in (d?.fields ?? {}))),
  );
  /** Auto-maintained bookkeeping: shown, never editable (the engine refuses too). */
  const AUTO = new Set(['blocked_from']);

  /**
   * The status menu.
   *
   * `allowed_statuses` leaves out terminal statuses on purpose — `set-status`
   * refuses them — but the document might currently *be* in one, or in a status
   * the type no longer declares. Prepending it keeps the menu from rendering
   * blank; choosing it again is a no-op.
   */
  const statuses = $derived.by(() => {
    const allowed = d?.allowed_statuses ?? [];
    if (!d?.status || allowed.includes(d.status)) return allowed;
    return [d.status, ...allowed];
  });

  // The write path.
  /** A label while a write is in flight; also the "one at a time" latch. */
  let pending = $state(null);
  /** The last refusal, shown verbatim — the node's messages are written for people. */
  let problem = $state(null);
  /** Kept so a 503 can be retried unchanged; the node's answer is "try again". */
  let lastAttempt = $state(null);

  // Status. Choosing one writes it at once — the same gesture as dragging a
  // card between columns — and a refusal for want of a `<status>_reason`
  // (blocked with no blocker, wontfix, whatever the project's rules say) opens
  // a reason box on the row, holding the picked status, so the second attempt
  // is one line of typing rather than a second trip through the menu.
  /** `{ status, reason }` while a refused change waits for its reason. */
  let retry = $state(null);
  let statusEl = $state(null);

  async function pickStatus(status) {
    if (!d || !status || status === d.status) return;
    const outcome = await perform(
      { action: 'set-status', id: d.id, status },
      'changing the status',
    );
    if (!outcome && problem?.invalid) {
      retry = { status, reason: '' };
    } else if (!outcome && statusEl) {
      // A one-way menu: nothing changed, so it must point back at the truth.
      statusEl.value = d.status;
    }
  }

  async function applyWithReason() {
    if (!retry) return;
    const reason = retry.reason.trim();
    const outcome = await perform(
      { action: 'set-status', id: d.id, status: retry.status, reason: reason || undefined },
      'changing the status',
    );
    if (outcome) retry = null;
  }

  function cancelRetry() {
    retry = null;
    if (statusEl && d) statusEl.value = d.status;
  }

  // The status the row was last seeded from. A plain variable, not `$state`:
  // nothing renders it. When the document's status really moves (this
  // reader's change landing, or anyone else's) a pending retry is stale and
  // drops; a mere refresh with the same status leaves a half-typed reason
  // alone.
  let seeded = null;
  $effect(() => {
    const current = d?.status;
    if (current === seeded) return;
    seeded = current;
    retry = null;
  });

  // Tags and blockers: an inline box at the end of each row's chips. Enter
  // writes; Escape clears the box.
  let newTags = $state('');
  let blocker = $state('');

  async function addTags() {
    const tags = splitCsv(newTags);
    if (tags.length === 0) return;
    const outcome = await perform({ action: 'tag', id: d.id, add: tags.join(',') }, 'adding tags');
    if (outcome) newTags = '';
  }

  async function addBlocker() {
    const by = blocker.trim();
    if (by.length === 0) return;
    const outcome = await perform({ action: 'block', id: d.id, by }, 'adding a blocker');
    if (outcome) blocker = '';
  }

  /** Enter submits, Escape clears — for the inline boxes. */
  function inlineKeys(submit, clear) {
    return (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        submit();
      } else if (event.key === 'Escape') {
        clear();
      }
    };
  }

  // Close: the one write that needs a second look, because it deletes the
  // file. The confirmation opens in place of the button.
  let confirmingClose = $state(false);
  let force = $state(false);

  async function close() {
    const outcome = await perform({ action: 'close', id: d.id, force }, 'closing');
    // On success the view navigates away; on refusal the dialog closes so the
    // message underneath it is not hidden behind a confirmation.
    confirmingClose = false;
    force = false;
    return outcome;
  }

  // Edit-in-place (the `edit-body` action): the rendered body swaps for its
  // markdown source. The draft seeds from the payload when editing starts and
  // is otherwise untouched by refreshes — a live reload must not eat an edit
  // in progress.
  let editing = $state(false);
  let draft = $state('');
  /** The body as editing began — the "untouched" reference for Escape. */
  let seed = '';
  /** The source textarea, for focus and caret placement on entry. */
  let srcEl = $state(null);

  function startEdit() {
    seed = d?.body ?? '';
    draft = seed;
    editing = true;
  }

  async function editFromButton() {
    startEdit();
    await tick();
    placeCaret(null);
  }

  /**
   * Enter editing from a click in the rendered prose — the body is editable by
   * default, Obsidian-style: the click swaps rendering for source and drops
   * the caret at the section that was clicked.
   *
   * Deliberately inert when the click was really something else: a link
   * (navigation wins), a selection being finished (copying is not editing), or
   * a modified click.
   */
  async function editFromClick(event) {
    if (editing || pending) return;
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    if (event.target.closest?.('a')) return;
    const selection = window.getSelection?.();
    if (selection && !selection.isCollapsed) return;
    const heading = nearestHeading(event.target, event.currentTarget);
    startEdit();
    await tick();
    placeCaret(heading);
  }

  /**
   * The heading governing the clicked spot: the clicked heading itself, or the
   * nearest one above the clicked block. Headings survive rendering almost
   * verbatim (`## Title` → `<h2>Title</h2>`), which is what makes them usable
   * for mapping a click in the HTML back to a line of the source.
   */
  function nearestHeading(target, article) {
    const el = target instanceof Element ? target : target?.parentElement;
    const own = el?.closest?.('h1,h2,h3,h4,h5,h6');
    if (own) return own;
    let block = el;
    while (block && block.parentElement !== article) block = block.parentElement;
    for (let sib = block; sib; sib = sib.previousElementSibling) {
      if (/^H[1-6]$/.test(sib.tagName)) return sib;
    }
    return null;
  }

  /** Focus the editor with the caret on `heading`'s source line (top if unknown). */
  function placeCaret(heading) {
    if (!srcEl) return;
    let at = 0;
    const text = heading?.textContent?.trim();
    if (text) {
      // A heading with inline markup (`code`, links) renders to different text
      // than its source line, so fall through: heading-line match, then the
      // bare text anywhere, then the top of the document.
      const escaped = text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      const line = draft.match(new RegExp(`^#{1,6}[ \\t]+${escaped}[ \\t]*$`, 'm'));
      at = line?.index ?? Math.max(0, draft.indexOf(text));
    }
    srcEl.focus();
    srcEl.setSelectionRange(at, at);
    // A textarea does not scroll to a programmatically placed caret on its
    // own; proportional is close enough to land the right screenful.
    srcEl.scrollTop = Math.max(
      0,
      (at / Math.max(1, draft.length)) * srcEl.scrollHeight - srcEl.clientHeight / 3,
    );
  }

  function editorKeys(event) {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      saveBody();
    } else if (event.key === 'Escape' && draft === seed) {
      // Escape discards only an untouched draft; once there are changes in
      // the box, the explicit Cancel is the only way to lose them.
      editing = false;
    }
  }

  // Frontmatter fields, editable in place (the `set-field`/`remove-field`
  // actions — verify-gated like the body, so the closed-frontmatter invariant
  // and the declared types refuse with their own messages).
  /** The row being edited: `{ key, text }`, or null. */
  let fieldEdit = $state(null);
  let newField = $state({ key: '', value: '' });

  /**
   * The current value as text the engine will read back to the same value.
   * The engine parses field text as YAML-scalar-else-string (the CLI's
   * `--field` coercion), so a string that *looks* like YAML data needs its
   * quotes; JSON is valid YAML, so stringify covers every non-string too.
   */
  function fieldSource(value) {
    if (typeof value === 'string') {
      const t = value.trim();
      if (t === '' || /^(true|false|null|~)$/i.test(t) || /^[-\d.[{'"&*?|>%@`]/.test(t)) {
        return JSON.stringify(value);
      }
      return value;
    }
    return JSON.stringify(value);
  }

  /** A value as the row shows it: a timestamp formatted, anything else as text. */
  function fieldDisplay(value) {
    return isTimestamp(value) ? shortTime(value) : fieldText(value);
  }

  function startFieldEdit(key, value) {
    if (pending || AUTO.has(key)) return;
    fieldEdit = { key, text: fieldSource(value) };
  }

  async function saveField() {
    if (!fieldEdit) return;
    const { key, text } = fieldEdit;
    // Clearing a field is removing it — the Obsidian gesture. A required
    // field's removal comes back as verify's "required field missing".
    const body =
      text.trim() === ''
        ? { action: 'remove-field', id: d.id, key }
        : { action: 'set-field', id: d.id, key, value: text };
    const outcome = await perform(body, 'saving a field');
    if (outcome) fieldEdit = null;
  }

  function fieldKeys(event) {
    if (event.key === 'Enter') {
      event.preventDefault();
      saveField();
    } else if (event.key === 'Escape') {
      fieldEdit = null;
    }
  }

  async function addField() {
    const key = newField.key.trim();
    if (!key) return;
    const outcome = await perform(
      { action: 'set-field', id: d.id, key, value: newField.value },
      'adding a field',
    );
    if (outcome) newField = { key: '', value: '' };
  }

  /** Focus (and select, so typing replaces) a just-mounted field input. */
  function focusOnMount(node) {
    node.focus();
    node.select?.();
  }

  async function saveBody() {
    const outcome = await perform(
      { action: 'edit-body', id: d.id, body: draft },
      'saving the body',
    );
    // On refusal the editor stays open with the draft intact — the node's
    // message (shown below the panel) says which rule the edit broke.
    if (outcome) editing = false;
  }

  /**
   * Send one action, then reconcile the view with what happened.
   *
   * Returns the node's outcome, or `null` if it refused — callers use that to
   * decide whether to clear their inputs.
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

  function retryLast() {
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

<div class="docview">
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
  <div class="topbar">
    <p class="crumb small page">
      <a href={href(boardPath(cid))}>← board</a>
      <span class="muted mono">{d.path}</span>
      {#if pending}
        <!-- The one place a write in flight is announced, so the whole panel
             below stays put while it waits. -->
        <span
          class="pendingtag tbside"
          title="a write waits for the inventory lock, which can take a few seconds if another opys command is running"
        >
          <span class="spinner" aria-hidden="true"></span>
          {pending}…
        </span>
      {:else if d.status}
        {@const tbtone = statusTone(d.status)}
        <span
          class="chip status tbside"
          style:--tone={tbtone}
          class:neutral={tbtone === null}>{d.status}</span
        >
      {/if}
    </p>
  </div>

  <header class="head page">
    <h1>{d.title || d.id}</h1>
    <div class="chips">
      <span class="chip mono">{d.id}</span>
      {#if d.type}
        {@const ttone = typeTone(d.type)}
        <span class="chip status" style:--tone={ttone} class:neutral={ttone === null}>
          {d.type}
        </span>
      {/if}
      {#if d.status}
        {@const tone = statusTone(d.status)}
        <span class="chip status" style:--tone={tone} class:neutral={tone === null}>
          {d.status}
        </span>
      {/if}
      <!-- The only place `updated` is rendered (it is in RESERVED): relative,
           with the whole timestamp in the tooltip. -->
      {#if d.updated}
        <span class="small muted" title={`updated ${shortTime(d.updated)}`}>
          updated {relativeTime(d.updated, now)}
        </span>
      {/if}
    </div>
  </header>

  {#if doc.error}
    <div class="notice warn page"><p>Could not refresh: {doc.error.message}</p></div>
  {/if}

  <section class="meta panel page">
    <div class="row">
      <span class="label microlabel">Status</span>
      {#if statuses.length === 0}
        <span class="small muted">
          No status can be set: this id's prefix matches no type the project
          declares.
        </span>
      {:else}
        <span class="fieldedit">
          <!-- One-way: the selection is a request, and the menu points back
               at the document's status unless the request is waiting on a
               reason. -->
          <select
            bind:this={statusEl}
            value={retry?.status ?? d.status}
            disabled={Boolean(pending)}
            aria-label="status"
            onchange={(e) => pickStatus(e.currentTarget.value)}
          >
            {#each statuses as status (status)}
              <option value={status}>{status}</option>
            {/each}
          </select>
          {#if retry}
            <input
              type="text"
              bind:value={retry.reason}
              placeholder={`reason for ${retry.status}`}
              aria-label={`reason for ${retry.status}`}
              use:focusOnMount
              onkeydown={inlineKeys(applyWithReason, cancelRetry)}
            />
            <button class="btn small primary" disabled={Boolean(pending)} onclick={applyWithReason}>
              Apply
            </button>
            <button class="btn small" onclick={cancelRetry}>Cancel</button>
          {/if}
        </span>
      {/if}
    </div>

    <div class="row">
      <span class="label microlabel">Tags</span>
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
        {/each}
        <input
          class="inline"
          type="text"
          bind:value={newTags}
          placeholder="+ tag"
          title="a tag, or several separated by commas — Enter adds"
          aria-label="add tags"
          disabled={Boolean(pending)}
          spellcheck="false"
          autocomplete="off"
          onkeydown={inlineKeys(addTags, () => (newTags = ''))}
        />
      </div>
    </div>

    {#each RELATIONS as relation (relation.key)}
      {@const entries = Object.entries(d[relation.key] ?? {})}
      {#if entries.length > 0 || relation.editable}
        <div class="row">
          <span class="label microlabel">{relation.label}</span>
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
              {:else if relation.editable}
                <span class="chip">
                  <a class="reflink" href={href(docPath(cid, refId))}>
                    <span class="mono">{refId}</span>
                    {shown.text}
                  </a>
                  <button
                    class="x"
                    title={`unblock: ${refId} no longer blocks ${d.id} (both sides are rewritten)`}
                    aria-label={`remove the blocker ${refId}`}
                    disabled={Boolean(pending)}
                    onclick={() =>
                      perform({ action: 'unblock', id: d.id, by: refId }, 'removing a blocker')}
                  >
                    ×
                  </button>
                </span>
              {:else}
                <a class="chip" href={href(docPath(cid, refId))}>
                  <span class="mono">{refId}</span>
                  {shown.text}
                </a>
              {/if}
            {/each}
            {#if relation.editable}
              <input
                class="inline mono"
                type="text"
                bind:value={blocker}
                placeholder="+ blocker id"
                title={`the id of a document that blocks ${d.id}; both sides are written — Enter adds`}
                aria-label="add a blocker by id"
                disabled={Boolean(pending)}
                spellcheck="false"
                autocomplete="off"
                onkeydown={inlineKeys(addBlocker, () => (blocker = ''))}
              />
            {/if}
          </div>
        </div>
      {/if}
    {/each}

    {#each custom as [key, value] (key)}
      {@const spec = declared.get(key)}
      <div class="row">
        <span class="label microlabel" title={spec?.description ?? key}>{key.replaceAll('_', ' ')}</span>
        {#if fieldEdit?.key === key}
          <span class="fieldedit">
            {#if spec?.type === 'enum'}
              <!-- The declared vocabulary as a select; a legacy value outside
                   it stays choosable so the row opens showing the truth. -->
              <select
                bind:value={fieldEdit.text}
                onchange={saveField}
                onkeydown={fieldKeys}
                use:focusOnMount
              >
                {#each spec.values.includes(fieldEdit.text) ? spec.values : [fieldEdit.text, ...spec.values] as v (v)}
                  <option value={v}>{v}</option>
                {/each}
              </select>
            {:else}
              <input
                type="text"
                bind:value={fieldEdit.text}
                onkeydown={fieldKeys}
                placeholder={spec?.pattern ?? ''}
                use:focusOnMount
                aria-label={`value of ${key}`}
              />
            {/if}
            <button class="btn small" disabled={Boolean(pending)} onclick={saveField}>
              Save
            </button>
            <button class="btn small" onclick={() => (fieldEdit = null)}>Cancel</button>
          </span>
        {:else if AUTO.has(key)}
          <span class="value" title={isTimestamp(value) ? value : undefined}>{fieldDisplay(value)}</span>
        {:else}
          <span class="value editable">
            <button
              class="valuebtn"
              disabled={Boolean(pending)}
              title={isTimestamp(value) ? `${value} — click to edit` : `edit ${key}`}
              onclick={() => startFieldEdit(key, value)}
            >
              {fieldDisplay(value)}
            </button>
            {#if !spec?.required}
              <button
                class="x"
                title={`remove the field "${key}"`}
                aria-label={`remove the field ${key}`}
                disabled={Boolean(pending)}
                onclick={() =>
                  perform({ action: 'remove-field', id: d.id, key }, 'removing a field')}
              >
                ×
              </button>
            {/if}
          </span>
        {/if}
      </div>
    {/each}

    <div class="row">
      <span class="label microlabel">+ field</span>
      <span class="fieldedit">
        <input
          class="fkey"
          type="text"
          placeholder="key"
          list="declared-keys"
          bind:value={newField.key}
          onkeydown={(e) => e.key === 'Enter' && addField()}
          aria-label="new field key"
        />
        <datalist id="declared-keys">
          {#each unsetDeclared as f (f.name)}
            <option value={f.name}>{f.description ?? f.type}</option>
          {/each}
        </datalist>
        <input
          type="text"
          placeholder="value"
          bind:value={newField.value}
          onkeydown={(e) => e.key === 'Enter' && addField()}
          aria-label="new field value"
        />
        <button
          class="btn small"
          disabled={Boolean(pending) || !newField.key.trim()}
          onclick={addField}
        >
          Add
        </button>
      </span>
    </div>

    <div class="row closerow">
      <span class="label microlabel">Close</span>
      {#if !d.closable}
        <span class="small muted">
          Never, for a <code>{d.type}</code>: the type declares no terminal
          status. Set one instead — archived, superseded, whatever the project
          uses.
        </span>
      {:else if !confirmingClose}
        <span class="fieldedit">
          <button class="btn small danger" disabled={Boolean(pending)} onclick={() => (confirmingClose = true)}>
            Close {d.id}
          </button>
          <span class="small muted">deletes the file</span>
        </span>
      {:else}
        <div class="confirm">
          <p>
            Close <strong>{d.id}</strong>{d.title ? ` — ${d.title}` : ''}?
          </p>
          <p class="small muted">
            This <strong>deletes</strong> <code>{d.path}</code> and strikes every
            reference to it into a tombstone. The document stops existing; only
            version control can bring it back.
          </p>
          <label class="small">
            <input type="checkbox" bind:checked={force} disabled={Boolean(pending)} />
            Close even if a required checklist still has unchecked items
          </label>
          <div class="confirmrow">
            <button class="btn small danger" disabled={Boolean(pending)} onclick={close}>
              Yes, close and delete
            </button>
            <button class="btn small" disabled={Boolean(pending)} onclick={() => (confirmingClose = false)}>
              Cancel
            </button>
          </div>
        </div>
      {/if}
    </div>
  </section>

  {#if problem}
    <div class="notice bad page">
      <!-- Verbatim. The node's refusals name the rule that was broken and often
           the exact command to satisfy it; a rewrite here would lose that. -->
      <p>{problem.message}</p>
      {#if problem.busy}
        <p class="why">
          Nothing was written. Another <code>opys</code> invocation was holding the
          inventory lock — the same request will work once it lets go.
        </p>
        <p><button class="btn small" onclick={retryLast}>Retry</button></p>
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
  <div class="page bodybar">
    {#if !editing}
      <button class="btn small" disabled={Boolean(pending)} onclick={editFromButton}>
        <Icon name="doc" size={13} /> Edit
      </button>
      <span class="small muted">or click anywhere in the text</span>
    {:else}
      <button class="btn primary small" disabled={Boolean(pending)} onclick={saveBody}>
        Save
      </button>
      <button class="btn small" disabled={Boolean(pending)} onclick={() => (editing = false)}>
        Cancel
      </button>
      <span class="small muted">
        markdown source · <kbd>⌘↵</kbd> saves — only if <code>verify</code> stays clean
      </span>
    {/if}
  </div>

  {#if editing}
    <!-- Obsidian's source mode, not a rich editor: the file is markdown and
         the person editing it knows that. -->
    <textarea
      class="page srcedit"
      bind:this={srcEl}
      bind:value={draft}
      onkeydown={editorKeys}
      rows={Math.min(40, Math.max(12, draft.split('\n').length + 2))}
      spellcheck="false"
      autocapitalize="off"
      autocorrect="off"
      aria-label="The document's markdown body"
    ></textarea>
  {:else}
    <!-- The click opens the editor; the Edit button above is the keyboard path
         to the same place, so the article itself stays a plain region. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <article class="body page" use:markdownLinks onclick={editFromClick}>
      {@html d.body_html}
    </article>
  {/if}
{/if}
</div>

<style>
  /* The reading measure. 50rem ≈ 75ch of body text: the Obsidian/typewriter
     sweet spot. Working views stay fluid; a document is for reading. */
  .docview {
    --page-w: 50rem;
  }

  .bodybar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    /* margin-block only: the shorthand would zero the inline margins and undo
       `.page`'s centering — the bar sat at the viewport's left edge while the
       text it belongs to was centered. */
    margin-block: 0.25rem 0.5rem;
  }

  .srcedit {
    display: block;
    width: 100%;
    font-family: var(--font-mono);
    font-size: 0.9rem;
    line-height: 1.55;
    resize: vertical;
    white-space: pre-wrap;
    padding: 0.8rem 0.9rem;
  }

  .tbside {
    margin-left: auto;
  }

  .pendingtag {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--muted);
  }

  .head {
    display: grid;
    gap: 0.4rem;
    margin-bottom: 1rem;
  }

  h1 {
    margin: 0;
    font-size: 1.55rem;
    font-weight: 650;
    letter-spacing: -0.015em;
    overflow-wrap: anywhere;
  }

  .meta {
    display: grid;
    padding: 0.25rem 0.85rem;
    margin-bottom: 1rem;
  }

  .row {
    display: grid;
    grid-template-columns: 7rem minmax(0, 1fr);
    gap: 0.5rem;
    align-items: baseline;
    padding: 0.45rem 0;
  }

  /* Hairline dividers between rows, softer than the panel border, so the panel
     reads as one card of facts rather than a stack of strips. */
  .row + .row {
    border-top: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  }

  @media (max-width: 34rem) {
    .row {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  .value {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .editable {
    display: inline-flex;
    gap: 0.35rem;
    align-items: baseline;
  }

  /* The value itself is the edit control, styled as the text it shows; the
     dotted underline on hover is the "this is editable" whisper. */
  .valuebtn {
    border: none;
    background: none;
    padding: 0;
    margin: 0;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: text;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .valuebtn:hover:not(:disabled) {
    text-decoration: underline dotted var(--muted);
    text-underline-offset: 3px;
  }

  .fieldedit {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .fieldedit input,
  .fieldedit select {
    font-size: 0.875rem;
    padding: 0.2rem 0.5rem;
    min-height: 1.75rem;
  }

  .fieldedit select {
    padding-right: 1.7rem;
  }

  .fieldedit input {
    flex: 1 1 10rem;
    min-width: 8rem;
  }

  .fieldedit input.fkey {
    flex: 0 1 10rem;
    font-family: var(--font-mono);
  }

  /* The inline box at the end of a chip row: a ghost chip that takes typing.
     It has the chips' shape so the row reads as "these, and room for one
     more", and lights up only when it is being used. */
  .inline {
    min-height: 0;
    width: 7.5rem;
    padding: 0.02rem 0.55rem;
    font-size: 0.8em;
    line-height: 1.5;
    border: 1px dashed var(--border-strong);
    border-radius: 999px;
    background: transparent;
    box-shadow: none;
    color: var(--fg);
  }

  .inline::placeholder {
    color: var(--muted);
    opacity: 0.8;
  }

  .inline:focus-visible {
    border-style: solid;
    width: 12rem;
  }

  .inline:disabled {
    opacity: 0.5;
  }

  .chip .reflink {
    color: inherit;
    text-decoration: none;
  }

  .chip .reflink:hover {
    text-decoration: underline;
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

  .closerow .small.muted {
    font-size: 0.78rem;
  }

  .confirm {
    border: 1px solid var(--bad);
    border-radius: 6px;
    padding: 0.6rem 0.7rem;
    display: grid;
    gap: 0.4rem;
  }

  .confirm p {
    margin: 0;
  }

  .confirm label {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .confirmrow {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  /* The body is server-rendered HTML, so its styles have to reach into it — the
     one place in this app where a global selector is the right tool. Scoped to
     `.body`, so nothing here can leak into the UI's own markup. */
  .body {
    overflow-wrap: anywhere;
    font-size: 1.06rem;
    line-height: 1.68;
    padding-bottom: 2rem;
    /* The prose is click-to-edit, and the I-beam is the honest cursor for
       "this text can be typed into". Links inside still show the pointer. */
    cursor: text;
  }

  .body :global(h1:first-child) {
    /* The document's own title, already in the header above. */
    display: none;
  }

  /* Section headings wear their markdown: a muted `##` marker in front. The
     `/ ''` alternative-text syntax marks it decorative so screen readers skip
     it; a browser too old for that syntax drops the whole declaration and the
     heading is simply unadorned — a correct degradation for pure decoration. */
  .body :global(h2) {
    font-size: 1.35rem;
    font-weight: 650;
    letter-spacing: -0.01em;
    margin: 2em 0 0.5em;
    padding-bottom: 0.25rem;
    border-bottom: 1px solid var(--border);
  }

  .body :global(h2)::before {
    content: '## ' / '';
    font-family: var(--font-mono);
    color: var(--accent);
    opacity: 0.55;
    font-weight: 400;
  }

  .body :global(h3) {
    font-size: 1.12rem;
    font-weight: 600;
    margin: 1.5em 0 0.4em;
  }

  .body :global(h3)::before {
    content: '### ' / '';
    font-family: var(--font-mono);
    color: var(--accent);
    opacity: 0.45;
    font-weight: 400;
  }

  .body :global(p),
  .body :global(ul),
  .body :global(ol) {
    margin: 0.8em 0;
  }

  .body :global(ul),
  .body :global(ol) {
    padding-left: 1.5em;
  }

  .body :global(li) {
    margin: 0.3em 0;
  }

  .body :global(li > ul),
  .body :global(li > ol) {
    margin: 0.2em 0;
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
    accent-color: var(--good);
  }
</style>
