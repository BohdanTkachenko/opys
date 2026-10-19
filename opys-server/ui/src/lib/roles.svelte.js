// The role directory and remembered choice are shared by the page and badge.
// Storage can be denied even when the browser exposes localStorage.
import { api } from './api.js';
import { corpora } from './corpora.svelte.js';
import { events } from './events.svelte.js';
import { affectsQueues } from './queue.js';
import { createResource } from './resource.svelte.js';

const ROLE_KEY = 'opys:role';
function readRole() {
  try {
    return localStorage.getItem(ROLE_KEY) || null;
  } catch {
    return null;
  }
}

function createRoles() {
  const resource = createResource();
  let remembered = $state(readRole());
  return {
    get all() { return resource.data?.roles ?? []; },
    get errors() { return resource.data?.errors ?? []; },
    get error() { return resource.error; },
    get settled() { return resource.settled; },
    get remembered() { return remembered; },
    get selected() { return this.all.find((role) => role.name === remembered); },
    remember(role) {
      remembered = role;
      try {
        localStorage.setItem(ROLE_KEY, role);
      } catch {
        // Keep the choice for this page's life when persistence is unavailable.
      }
    },
    reload(quiet = false) { return resource.run(() => api.roles(), { quiet }); },
    start() {
      this.reload();
      return events.subscribe((batch) => {
        if (affectsQueues(batch, corpora.groups)) this.reload(true);
      });
    },
  };
}

export const roles = createRoles();
