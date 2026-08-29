<script>
  // The action bar: the four writes a document view offers.
  //
  // Everything here is driven by the payload rather than by knowledge of the
  // project's config. The status menu is `allowed_statuses` — the type's
  // statuses minus its terminal ones, computed by the node — and the close
  // button is gated on `closable`, because a type with no terminal status has
  // nothing to close *to* and the engine refuses no matter what the reader does.
  // A UI that derived either from `opys.toml` would be a second interpretation
  // of the config, wrong the first time a type was edited.
  //
  // The component holds only its own inputs. The write itself, the pending latch
  // and the refusal message belong to the view, which passes `perform` in.

  import { splitCsv } from './lib/format.js';

  let { doc, pending, perform } = $props();

  /**
   * The status menu.
   *
   * `allowed_statuses` leaves out terminal statuses on purpose — `set-status`
   * refuses them — but the document might currently *be* in one, or in a status
   * the type no longer declares. Prepending it keeps the menu from rendering
   * blank; applying it is impossible anyway, because "apply" is disabled while
   * the selection equals the current status.
   */
  const statuses = $derived.by(() => {
    const allowed = doc.allowed_statuses ?? [];
    if (!doc.status || allowed.includes(doc.status)) return allowed;
    return [doc.status, ...allowed];
  });

  // The menu shows where the document is until the reader points it somewhere
  // else, and follows the document again once its status actually changes.
  //
  // Real state plus a fallback, *not* an overridable `$derived`: an override on
  // a derived survives only until a dependency changes, and the dependency here
  // is the identity of the payload object, not the status string. The doc view
  // reloads on every event affecting this corpus — any CLI write anywhere in the
  // inventory, any other tab, the reader's own next tag edit — so a derived
  // would silently drop a selection and disable Apply, with the typed reason
  // still sitting there next to an unchanged status.
  // The `includes` is for the one way a selection can go stale without the
  // document moving: the project's config is edited and the status stops being
  // offered. Falling back then keeps the menu from rendering blank.
  let picked = $state(null);
  const nextStatus = $derived(picked !== null && statuses.includes(picked) ? picked : doc.status);

  // The status the menu was last seeded from. A plain variable, not `$state`:
  // nothing renders it, and reading reactive state inside the effect that writes
  // it would re-trigger the effect. It starts as a sentinel rather than
  // `doc.status` so the first read of the payload happens inside the effect,
  // where it is a dependency.
  let seeded = null;
  $effect(() => {
    // Reading `doc.status` here subscribes to the payload, so this runs on every
    // refresh — the comparison is what makes it a no-op unless the status really
    // moved (someone else set it, or this reader's own change landed).
    const current = doc.status;
    if (current === seeded) return;
    seeded = current;
    picked = null;
  });

  let reason = $state('');
  let newTags = $state('');
  let blocker = $state('');
  let confirmingClose = $state(false);
  let force = $state(false);

  const busy = $derived(Boolean(pending));

  async function applyStatus() {
    const outcome = await perform(
      {
        action: 'set-status',
        id: doc.id,
        status: nextStatus,
        // Some statuses require a `<status>_reason`; which ones is a project
        // rule, so the field is always offered and never required here. The
        // node's refusal says exactly which rule was missed.
        reason: reason.trim().length > 0 ? reason.trim() : null,
      },
      'changing the status',
    );
    if (outcome) reason = '';
  }

  async function addTags() {
    const tags = splitCsv(newTags);
    if (tags.length === 0) return;
    const outcome = await perform(
      { action: 'tag', id: doc.id, add: tags.join(',') },
      'adding tags',
    );
    if (outcome) newTags = '';
  }

  async function block(action) {
    const by = blocker.trim();
    if (by.length === 0) return;
    const outcome = await perform({ action, id: doc.id, by }, `${action}ing`);
    if (outcome) blocker = '';
  }

  async function close() {
    const outcome = await perform({ action: 'close', id: doc.id, force }, 'closing');
    // On success the view navigates away; on refusal the dialog closes so the
    // message underneath it is not hidden behind a confirmation.
    confirmingClose = false;
    force = false;
    return outcome;
  }
</script>

<section class="actions panel">
  <div class="group">
    <span class="label small muted">Status</span>
    {#if statuses.length === 0}
      <span class="small muted">
        No status can be set: this id's prefix matches no type the project
        declares.
      </span>
    {:else}
      <!-- One-way, like the board's facet menus: the selection is state this
           component owns, and `value` only says where it currently points. -->
      <select
        value={nextStatus}
        disabled={busy}
        onchange={(e) => (picked = e.currentTarget.value)}
      >
        {#each statuses as status (status)}
          <option value={status}>{status}</option>
        {/each}
      </select>
      <input
        type="text"
        bind:value={reason}
        placeholder="reason (some statuses need one)"
        disabled={busy}
      />
      <button
        class="btn primary"
        disabled={busy || nextStatus === doc.status}
        onclick={applyStatus}
      >
        Apply
      </button>
    {/if}
  </div>

  <div class="group">
    <span class="label small muted">Tags</span>
    <input
      type="text"
      bind:value={newTags}
      placeholder="tag, another-tag"
      disabled={busy}
      onkeydown={(e) => {
        if (e.key === 'Enter') addTags();
      }}
    />
    <button class="btn" disabled={busy || splitCsv(newTags).length === 0} onclick={addTags}>
      Add
    </button>
    <span class="small muted">remove a tag with the × on its chip above</span>
  </div>

  <div class="group">
    <span class="label small muted">Blockers</span>
    <input
      type="text"
      bind:value={blocker}
      placeholder="TASK-0001"
      disabled={busy}
      spellcheck="false"
      onkeydown={(e) => {
        if (e.key === 'Enter') block('block');
      }}
    />
    <button class="btn" disabled={busy || blocker.trim().length === 0} onclick={() => block('block')}>
      Add blocker
    </button>
    <button
      class="btn"
      disabled={busy || blocker.trim().length === 0}
      onclick={() => block('unblock')}
    >
      Remove blocker
    </button>
    <span class="small muted">
      the id of the document blocking <code>{doc.id}</code>; both sides are
      written
    </span>
  </div>

  <div class="group">
    <span class="label small muted">Close</span>
    {#if !doc.closable}
      <span class="small muted">
        Documents of type <code>{doc.type}</code> cannot be closed: the type
        declares no terminal status. Set a status instead — archived, superseded,
        whatever the project uses.
      </span>
    {:else if !confirmingClose}
      <button class="btn danger" disabled={busy} onclick={() => (confirmingClose = true)}>
        Close {doc.id}
      </button>
      <span class="small muted">this deletes the file</span>
    {:else}
      <div class="confirm">
        <p>
          Close <strong>{doc.id}</strong>{doc.title ? ` — ${doc.title}` : ''}?
        </p>
        <p class="small muted">
          This <strong>deletes</strong> <code>{doc.path}</code> and strikes every
          reference to it into a tombstone. The document stops existing; only
          version control can bring it back.
        </p>
        <label class="small">
          <input type="checkbox" bind:checked={force} disabled={busy} />
          Close even if a required checklist still has unchecked items
        </label>
        <div class="row">
          <button class="btn danger" disabled={busy} onclick={close}>
            Yes, close and delete
          </button>
          <button class="btn" disabled={busy} onclick={() => (confirmingClose = false)}>
            Cancel
          </button>
        </div>
      </div>
    {/if}
  </div>

  {#if busy}
    <p class="pending small">
      <span class="spinner" aria-hidden="true"></span>
      {pending}…
      <span class="muted">
        a write waits for the inventory lock, which can take a few seconds if
        another <code>opys</code> command is running
      </span>
    </p>
  {/if}
</section>

<style>
  .actions {
    display: grid;
    gap: 0.5rem;
    padding: 0.7rem 0.8rem;
    margin-bottom: 1rem;
  }

  .group {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
  }

  .label {
    flex: none;
    width: 6rem;
    text-transform: lowercase;
  }

  @media (max-width: 34rem) {
    .label {
      width: 100%;
    }
  }

  .confirm {
    flex: 1 1 20rem;
    border: 1px solid var(--bad);
    border-radius: 4px;
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

  .confirm .row {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .pending {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    margin: 0;
  }

  .spinner {
    width: 0.7rem;
    height: 0.7rem;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* Respect a reader who has asked for less movement; the text says the same
     thing on its own. */
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }
</style>
