// Role queues are already ranked by the engine. Keep its project, type and
// status order; grouping here only separates attention from ordinary work.

/** The same primary-corpus choice as the node's project-keyed routes. */
export function primaryCorpus(group) {
  return group?.corpora?.find((corpus) => corpus.is_primary) ?? group?.corpora?.[0];
}

/** Whether a coalesced event batch can change any primary role queue. */
export function affectsQueues(batch, groups) {
  if (batch.reconnected) return true;
  const primary = new Set(groups.map((group) => primaryCorpus(group)?.cid));
  return batch.events.some((event) => {
    if (event.event === 'refs-changed') return groups.some((group) => group.key === event.project);
    if (event.event === 'corpus-reloaded') return primary.has(event.cid);
    return event.cid === undefined || primary.has(event.cid) ||
      event.event === 'corpus-added' || event.event === 'corpus-removed';
  });
}

/**
 * A display model without invented empty queues for failed projects. Rows keep
 * the engine's order, including an inherited priority's position. An owned
 * empty group remains in the API but does not take space on the page.
 */
export function groupRoleQueues(view, groups = []) {
  const attention = [];
  const projects = [];
  for (const project of view?.projects ?? []) {
    const cid = primaryCorpus(groups.find((group) => group.key === project.key))?.cid ?? null;
    const entry = { ...project, cid };
    if ('error' in project) {
      projects.push(entry);
      continue;
    }
    const queue = project.queue;
    const waiting = (queue.groups ?? []).map((group) => ({ ...group, owners: group.owners ?? [], in_progress: group.in_progress ?? [] }))
      .filter((group) => group.total > 0 || group.items.length > 0 ||
        group.in_progress.length > 0 || group.blocked.length > 0 || group.orphaned.length > 0);
    if (queue.attention.length > 0 || queue.project_problems.length > 0) {
      attention.push({ ...entry, items: queue.attention, problems: queue.project_problems });
    }
    if (waiting.length > 0 || queue.notices?.length) projects.push({ ...entry, groups: waiting, notices: queue.notices ?? [] });
  }
  return { attention, projects, empty: attention.length === 0 && projects.length === 0 };
}

/** Elapsed time at the current step: full units, with the exact date in a tooltip. */
export function relativeSince(iso, now = Date.now()) {
  if (typeof iso !== 'string' || iso.length === 0) return null;
  const then = Date.parse(iso);
  if (!Number.isFinite(then)) return null;
  const minutes = Math.floor(Math.max(0, now - then) / 60_000);
  if (minutes === 0) return 'just now';
  if (minutes < 60) return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} ${hours === 1 ? 'hour' : 'hours'}`;
  const days = Math.floor(hours / 24);
  return `${days} ${days === 1 ? 'day' : 'days'}`;
}
