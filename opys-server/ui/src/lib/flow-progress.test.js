import assert from 'node:assert/strict';
import { test } from 'node:test';
import { affectsFlowProgress, buildFlowProgress, claimedFlowDocuments, filterFlowDocuments, flowDocumentPath, flowProgressCounts } from './flow-progress.js';
import { buildWorkflowMap } from './workflow-map.js';

const type = { name: 'task', prefix: 'TASK', statuses: ['todo', 'doing', 'review'], terminal_statuses: [], default_status: 'todo', has_graph: true };
const columns = [
  { cid: 'main-cid', label: 'main (primary)', kind: 'worktree' },
  { cid: 'work-cid', label: 'agent-codex', kind: 'worktree' },
  { cid: 'ref-123', label: 'agent-claude', kind: 'branch', commit: '123' },
  { cid: 'ref-456', label: 'agent-agy', kind: 'branch', commit: '456' },
];
const cell = (index, status, title = 'Ticket', extra = {}) => ({ cid: columns[index].cid, status, title, unknown: false, ...extra });
const view = (rows = [], extra = {}) => ({ cid: 'main-cid', types: [type], nodes: [], edges: [], progress: { columns, rows, notices: [], ...extra } });
const main = [{ id: 'TASK-0001', title: 'Ticket', status: 'todo', type: 'task', blocked: true, priority: 10 },
  { id: 'TASK-0002', title: 'Unchanged', status: 'review', type: 'task', blocked: false }];

test('declared_work_stages_replace_pool_copies_and_do_not_duplicate_branch_snapshots', () => {
  const input = view([
    { id: 'TASK-0001', cells: [cell(0, 'todo'), cell(1, null), cell(2, 'review'), cell(3, null)] },
    { id: 'TASK-0002', cells: [cell(0, 'review'), cell(1, null), cell(2, 'todo', 'Old pool snapshot'), cell(3, null)] },
  ]);
  input.progress.columns = columns.map((column, index) => index === 2 ? { ...column, label: 'spec/TASK-0001' } : column);
  input.documents = [{ ...main[0], status: 'review', ref: 'spec/TASK-0001', claim: { holder: 'worker-7' } }, main[1]];
  input.work_refs = ['spec/TASK-0001'];
  const result = buildFlowProgress(input, main);
  assert.deepEqual(result.documents.map((doc) => [doc.id, doc.status]), [['TASK-0001', 'review'], ['TASK-0002', 'review']]);
  assert.equal(result.documents[0].claim.holder, 'worker-7');
  assert.equal(result.documents[0].sources[0].label, 'spec/TASK-0001');
  assert.equal(result.documents[0].sources[0].work, true);
  assert.equal(flowDocumentPath(result.documents[0].sources[0], 'TASK-0001'), '/corpus/main-cid/doc/TASK-0001');
  assert.deepEqual(flowProgressCounts(result.documents), { tickets: 2, primary: 2, progress: 0 });
});

test('flow_progress_retains_all_main_documents_and_deduplicates_snapshots', () => {
  const input = view([{ id: 'TASK-0001', cells: [cell(0, 'todo'), cell(1, 'doing'), cell(2, 'doing'), cell(3, 'todo')] }]);
  const original = JSON.stringify(input);
  const result = buildFlowProgress(input, main);
  assert.equal(result.documents.length, 3);
  const truth = result.documents.filter((doc) => doc.primary);
  assert.deepEqual(truth.map((doc) => doc.id), ['TASK-0001', 'TASK-0002'], 'unchanged main rows absent from progress remain visible');
  assert.equal(truth[0].blocked, true);
  assert.equal(truth[0].priority, 10);
  const branch = result.documents.find((doc) => !doc.primary);
  assert.equal(branch.status, 'doing');
  assert.deepEqual(branch.sources.map((source) => source.label), ['agent-codex', 'agent-claude']);
  assert.equal(branch.blocked, null, 'branch blocker state is unknown, never borrowed from main');
  assert.deepEqual(flowProgressCounts(result.documents), { tickets: 2, primary: 2, progress: 1 });
  assert.equal(JSON.stringify(input), original);
});

test('flow_progress_keeps_concurrent_statuses_titles_and_branch_only_documents', () => {
  const result = buildFlowProgress(view([
    { id: 'TASK-0001', cells: [cell(0, 'todo'), cell(1, 'doing'), cell(2, 'review'), cell(3, 'review', 'A different approach')] },
    { id: 'TASK-0003', cells: [cell(0, null), cell(1, 'research', 'New ticket'), cell(2, 'research', 'New ticket'), cell(3, null)] },
  ]), main);
  assert.deepEqual(result.documents.filter((doc) => doc.id === 'TASK-0001').map((doc) => [doc.status, doc.title]),
    [['todo', 'Ticket'], ['doing', 'Ticket'], ['review', 'Ticket'], ['review', 'A different approach']]);
  const branchOnly = result.documents.find((doc) => doc.id === 'TASK-0003');
  assert.equal(branchOnly.primary, false);
  assert.equal(branchOnly.primaryCid, null);
  assert.equal(branchOnly.type, 'task');
  assert.equal(branchOnly.sources.length, 2);
  const model = buildWorkflowMap(view(), null, result.documents);
  assert.equal(model.nodes.find((node) => node.status === 'research').declared, false);
  assert.deepEqual(flowProgressCounts(result.documents), { tickets: 3, primary: 2, progress: 4 });
});

test('flow_progress_unknown_and_failed_sources_remain_errors_not_documents', () => {
  const failed = { ...columns[3], error: 'cannot read commit' };
  const result = buildFlowProgress(view([
    { id: 'TASK-0001', cells: [cell(0, 'todo'), cell(1, null, null, { unknown: true }), cell(2, null), cell(3, 'doing')] },
  ], { columns: [...columns.slice(0, 3), failed], notices: ['No remote refs configured'] }), main);
  assert.equal(result.documents.length, main.length);
  assert.equal(result.errors.length, 2);
  assert.ok(result.errors.some((error) => error.label === 'agent-agy' && error.error === 'cannot read commit'));
  assert.ok(result.errors.some((error) => error.label === 'agent-codex' && error.error.includes('unknown')));
  assert.deepEqual(result.notices, ['No remote refs configured']);
});

test('flow_progress_links_only_real_worktrees_and_never_ref_keys', () => {
  const result = buildFlowProgress(view([{ id: 'TASK-0001', cells: [cell(0, 'todo'), cell(1, 'doing'), cell(2, 'doing'), cell(3, 'review')] }]), main);
  const doing = result.documents.find((doc) => doc.status === 'doing');
  assert.equal(flowDocumentPath(doing.sources[0], doing.id), '/corpus/work-cid/doc/TASK-0001');
  assert.equal(flowDocumentPath(doing.sources[1], doing.id), null);
  assert.equal(doing.sources[1].cid, null);
  assert.equal(flowDocumentPath({ kind: 'branch', cid: 'ref-123' }, 'TASK-0001'), null);
  assert.equal(flowDocumentPath({ kind: 'worktree', cid: 'work/tree' }, 'TASK-0001'), '/corpus/work%2Ftree/doc/TASK-0001');
});

test('flow_progress_filters_apply_to_every_branch_version_and_source_label', () => {
  const result = buildFlowProgress(view([{ id: 'TASK-0001', cells: [cell(0, 'todo'), cell(1, 'doing'), cell(2, 'review'), cell(3, 'review', 'Alternative')] }]), main);
  assert.equal(filterFlowDocuments(result.documents, { hiddenTypes: ['task'] }).length, 0);
  assert.equal(filterFlowDocuments(result.documents, { status: 'review' }).length, 3);
  assert.equal(filterFlowDocuments(result.documents, { search: 'alternative' })[0].title, 'Alternative');
  assert.equal(filterFlowDocuments(result.documents, { search: 'agent-codex' })[0].status, 'doing');
});

test('flow_progress_refreshes_any_group_corpus_refs_roster_and_reconnect', () => {
  const group = { corpora: [{ cid: 'main-cid' }, { cid: 'work-cid' }] };
  const batch = (event) => ({ reconnected: false, events: [event] });
  for (const event of [{ event: 'corpus-reloaded', cid: 'main-cid' },
    { event: 'corpus-reloaded', cid: 'work-cid' }, { event: 'refs-changed', project: 'project' },
    { event: 'corpus-added', cid: 'new' }, { event: 'corpus-removed', cid: 'old' }]) {
    assert.equal(affectsFlowProgress(batch(event), 'project', group, view()), true);
  }
  assert.equal(affectsFlowProgress(batch({ event: 'corpus-reloaded', cid: 'another-project' }), 'project', group, view()), false);
  assert.equal(affectsFlowProgress(batch({ event: 'refs-changed', project: 'another-project' }), 'project', group, view()), false);
  assert.equal(affectsFlowProgress({ reconnected: true, events: [] }, 'project', group, view()), true);
});

test('flow_activity_uses_explicit_available_claims_not_stage_ownership_or_branch_presence', () => {
  const claim = { holder: 'worker-7', since: '2026-10-22T18:00:00Z' };
  const docs = [
    { id: 'TASK-0001', status: 'doing', claim, unavailable: 'Two work branches are open' },
    { id: 'TASK-0002', status: 'doing', ref: 'spec/TASK-0002', owners: ['agent-claude'] },
    { id: 'TASK-0003', status: 'doing', claim },
    { id: 'TASK-0004', status: 'doing', claim, unavailable: '' },
  ];
  assert.deepEqual(claimedFlowDocuments(docs).map((doc) => doc.id), ['TASK-0003']);
  assert.deepEqual(claimedFlowDocuments(docs.map(({ claim, ...doc }) => doc)), [], 'released cards leave the activity list');
});

test('flow_activity_respects_type_filters_and_preserves_unknown_claim_ages', () => {
  const result = buildFlowProgress({ ...view(), documents: [
    { id: 'TASK-0001', type: 'task', status: 'doing', claim: { holder: 'worker-task' } },
    { id: 'BUG-0002', type: 'bug', status: 'doing', claim: { holder: 'worker-bug', since: 'invalid' } },
  ] });
  const visible = filterFlowDocuments(result.documents, { hiddenTypes: ['bug'] });
  assert.deepEqual(claimedFlowDocuments(visible).map((doc) => doc.id), ['TASK-0001']);
  assert.equal(claimedFlowDocuments(visible)[0].claim.since, undefined);
  assert.equal(claimedFlowDocuments(result.documents).find((doc) => doc.id === 'BUG-0002').claim.since, 'invalid');
  assert.equal(claimedFlowDocuments(result.documents).find((doc) => doc.id === 'TASK-0001').claim.since, undefined);
  assert.equal(claimedFlowDocuments(filterFlowDocuments(result.documents, { hiddenTypes: ['bug', 'task'] })).length, 0);
});

test('flow_activity_deduplicates_copies_preserves_parallel_claims_and_does_not_move_the_graph', () => {
  const claim = { holder: 'worker-7', since: '2026-10-22T18:00:00Z' };
  const primary = { id: 'TASK-0001', type: 'task', status: 'doing', ref: 'spec/TASK-0001', claim, primary: true };
  const docs = [
    { ...primary, primary: false }, primary,
    { ...primary, ref: 'impl/TASK-0001', claim: { holder: 'worker-8', since: '2026-10-22T19:00:00Z' } },
  ];
  const before = JSON.stringify(docs);
  const active = claimedFlowDocuments(docs);
  assert.equal(active.length, 2);
  assert.equal(active[0], primary, 'prefer the authoritative served copy for its link');
  assert.equal(JSON.stringify(docs), before);
  assert.deepEqual(buildWorkflowMap(view(), null, docs), buildWorkflowMap(view(), null, docs.map(({ claim, ...doc }) => doc)),
    'claim updates preserve graph geometry and the user\'s viewport');
});
