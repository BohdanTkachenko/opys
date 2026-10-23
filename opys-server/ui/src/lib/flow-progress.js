// Main documents remain the truth. Branch cells add labelled versions; they
// never replace main, decide a winner, or manufacture a served corpus.
const versionKey = (doc) => JSON.stringify([doc.id, doc.status ?? '', doc.title ?? '']);

/** A real document route, only for a source backed by a served worktree. */
export function flowDocumentPath(source, id) {
  return source?.kind === 'worktree' && source.cid
    ? `/corpus/${encodeURIComponent(source.cid)}/doc/${encodeURIComponent(id)}` : null;
}

/** Merge identical branch versions while retaining every source and main doc. */
export function buildFlowProgress(view, primaryDocuments = []) {
  primaryDocuments = view?.documents ?? primaryDocuments;
  const workRefs = new Set(view?.work_refs ?? []);
  const columns = view?.progress?.columns ?? [];
  const primaryColumn = columns.find((column) => column.cid === view?.cid);
  const primarySource = { key: view?.cid, cid: view?.cid, label: primaryColumn?.label ?? 'Primary', kind: 'worktree', primary: true };
  const primary = new Map(primaryDocuments.map((doc) => [doc.id, doc]));
  const versions = new Map(primaryDocuments.map((doc) => {
    const key = versionKey(doc);
    const source = doc.ref ? { ...primarySource, label: doc.ref, work: true, primary: false } : primarySource;
    return [key, { ...doc, key, primary: true, sources: [source], primaryCid: view?.cid }];
  }));
  const errors = new Map(columns.filter((column) => column.error).map((column) => [column.cid,
    { key: column.cid, label: column.label, error: column.error }]));

  for (const row of view?.progress?.rows ?? []) {
    for (const [index, cell] of (row.cells ?? []).entries()) {
      const column = columns[index];
      if (!column || column.cid !== cell.cid) {
        errors.set(cell.cid, { key: cell.cid, label: cell.cid, error: 'Progress source could not be identified.' });
        continue;
      }
      if (cell.cid === view?.cid || column.error) continue;
      // A declared branch carries one work copy, already supplied by the
      // engine. Its older snapshots of every other ticket are not more work.
      if ([...workRefs].some((ref) => column.label === ref || column.label.startsWith(`${ref} (`))) continue;
      if (cell.unknown) {
        if (!errors.has(column.cid)) errors.set(column.cid, {
          key: column.cid, label: column.label, error: 'Some document states are unknown; no absence or progress is inferred from them.',
        });
        continue;
      }
      if (cell.status === null || cell.status === undefined) continue;
      const main = primary.get(row.id);
      const doc = {
        id: row.id, status: cell.status, title: cell.title ?? '',
        type: main?.type ?? view?.types?.find((type) => row.id.startsWith(`${type.prefix}-`))?.name ?? null,
      };
      const key = versionKey(doc);
      // Unchanged snapshots do not turn every branch into another copy of main.
      if (main && key === versionKey(main)) continue;
      const source = {
        key: column.cid, cid: column.kind === 'worktree' ? column.cid : null,
        label: column.label, kind: column.kind, commit: column.commit, primary: false,
      };
      let version = versions.get(key);
      if (!version) {
        version = { ...doc, key, primary: false, sources: [], primaryCid: main ? view.cid : null, blocked: null, priority: null };
        versions.set(key, version);
      }
      if (!version.sources.some((entry) => entry.key === source.key)) version.sources.push(source);
    }
  }
  return { documents: [...versions.values()], errors: [...errors.values()], notices: [...new Set([...(view?.notices ?? []), ...(view?.progress?.notices ?? [])])] };
}

/** Count tickets separately from the additional versions shown at branch tips. */
export function flowProgressCounts(documents) {
  return {
    tickets: new Set(documents.map((doc) => doc.id)).size,
    primary: documents.filter((doc) => doc.primary).length,
    progress: documents.filter((doc) => !doc.primary).length,
  };
}

/** Count blocker states once per ticket, using only authoritative documents. */
export function flowStatusCounts(documents) {
  const totals = flowProgressCounts(documents);
  const states = new Map();
  for (const doc of documents) {
    if (!states.has(doc.id)) states.set(doc.id, null);
    if (doc.primary && typeof doc.blocked === 'boolean') states.set(doc.id, doc.blocked);
  }
  const blocked = [...states.values()].filter((state) => state === true).length;
  const unblocked = [...states.values()].filter((state) => state === false).length;
  const unknown = totals.tickets - blocked - unblocked;
  return { ...totals, blocked, unblocked, unknown, label: `${unknown ? '?' : unblocked}/${totals.tickets}` };
}

/** Explicit claims, not status owners or branch snapshots, identify in-progress work. */
export function claimedFlowDocuments(documents) {
  const claims = new Map();
  for (const doc of documents) {
    if (!doc.claim || doc.unavailable != null) continue;
    const key = JSON.stringify([doc.id, doc.status, doc.ref ?? null, doc.claim.holder]);
    if (!claims.has(key) || doc.primary) claims.set(key, doc);
  }
  const activity = (doc) => {
    const time = Date.parse(doc.claim.since);
    return Number.isFinite(time) ? time : Infinity;
  };
  return [...claims.values()].sort((a, b) => activity(a) - activity(b) || a.id.localeCompare(b.id));
}

/** The same filters apply to primary documents and every branch version. */
export function filterFlowDocuments(documents, { hiddenTypes = [], search = '', status } = {}) {
  const needle = search.trim().toLowerCase();
  return documents.filter((doc) => !hiddenTypes.includes(doc.type) &&
    (status === undefined || doc.status === status) &&
    (!needle || [doc.id, doc.title, doc.type, doc.status, ...doc.sources.map((source) => source.label)]
      .join(' ').toLowerCase().includes(needle)));
}

/** Ref movement and writes in any member worktree can change branch progress. */
export function affectsFlowProgress(batch, key, group, view) {
  if (batch.reconnected) return true;
  const cids = new Set([view?.cid, ...(group?.corpora ?? []).map((corpus) => corpus.cid),
    ...(view?.progress?.columns ?? []).filter((column) => column.kind === 'worktree').map((column) => column.cid)]);
  cids.delete(undefined);
  return batch.events.some((event) => event.project === key ||
    (event.cid === undefined && event.project === undefined) ||
    cids.has(event.cid) || event.event === 'corpus-added' || event.event === 'corpus-removed');
}
