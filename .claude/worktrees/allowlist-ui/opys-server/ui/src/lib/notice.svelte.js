// A page-level notice, for something a view has to say after it stops existing.
//
// There is exactly one thing like that, and it is why this module exists:
// `close` deletes the document, so the doc view navigates to the board — but
// the node's answer to a close can carry `sync_skipped`, meaning the write
// landed and the reconcile/linkify/relocate pass did not. That is the *most*
// important time to say so (the deleted document's references were not struck,
// so the corpus now has dangling links), and the component that heard it is
// unmounted in the same tick. Component state cannot outlive that; this can.
//
// Deliberately not a queue and not a toast: one message, shown until it is
// dismissed, because a warning about an unmaintained corpus should not time out.

let message = $state(null);

export const notice = {
  /** The current message, or `null`. */
  get message() {
    return message;
  },
  /** Raise one. A nullish text clears, so `show(outcome.sync_skipped)` is safe. */
  show(text) {
    message = typeof text === 'string' && text.length > 0 ? text : null;
  },
  clear() {
    message = null;
  },
};
