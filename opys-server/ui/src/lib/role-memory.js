// Keep the cross-project choice separate from each project's opaque key.
function key(projectKey) {
  return projectKey == null ? 'opys:role' : `opys:role:project:${encodeURIComponent(projectKey)}`;
}

export function readRole(projectKey = null, storage) {
  try {
    return (storage ?? globalThis.localStorage).getItem(key(projectKey)) || null;
  } catch {
    return null;
  }
}

export function writeRole(role, projectKey = null, storage) {
  try {
    (storage ?? globalThis.localStorage).setItem(key(projectKey), role);
  } catch {
    // The reactive store keeps the choice for this page's life.
  }
}
