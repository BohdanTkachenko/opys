// The omnibox's state, shared so that any view's placeholder and the global
// shortcut open the same box. The box itself is `Omni.svelte`, mounted once
// by the shell.

let open = $state(false);
let cid = $state(null);

export const omni = {
  get open() {
    return open;
  },
  /** The corpus the box searches, or `null` for every served corpus. */
  get cid() {
    return cid;
  },
  show(scope = null) {
    cid = scope;
    open = true;
  },
  hide() {
    open = false;
  },
};

/** The modifier the shortcut hints name: ⌘ on Apple hardware, Ctrl elsewhere. */
export const MOD = /Mac|iPhone|iPad|iPod/.test(navigator.platform ?? '') ? '⌘' : 'Ctrl';
