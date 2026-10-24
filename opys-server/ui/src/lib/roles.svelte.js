// The role directory and remembered choice are shared by the page and badge.
// Storage can be denied even when the browser exposes localStorage.
import { api } from './api.js';
import { corpora } from './corpora.svelte.js';
import { events } from './events.svelte.js';
import { affectsQueues } from './queue.js';
import { createResource } from './resource.svelte.js';
import { readRole, writeRole } from './role-memory.js';

function createRoles() {
  const resource = createResource();
  let remembered = $state(readRole());
  let projectChoices = $state({});
  return {
    get all() { return resource.data?.roles ?? []; },
    get errors() { return resource.data?.errors ?? []; },
    get error() { return resource.error; },
    get settled() { return resource.settled; },
    get remembered() { return remembered; },
    get selected() { return this.all.find((role) => role.name === remembered); },
    rememberedFor(projectKey = null) {
      if (projectKey == null) return remembered;
      return Object.hasOwn(projectChoices, projectKey) ? projectChoices[projectKey] : readRole(projectKey);
    },
    remember(role, projectKey = null) {
      if (projectKey == null) remembered = role;
      else projectChoices = { ...projectChoices, [projectKey]: role };
      writeRole(role, projectKey);
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
