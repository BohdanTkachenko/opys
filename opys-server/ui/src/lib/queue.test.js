import assert from 'node:assert/strict';
import { test } from 'node:test';
import { affectsQueues, groupRoles, groupRoleQueues, primaryCorpus, projectRoles, relativeSince, scopeRoleView, selectRole } from './queue.js';

test('project roles use local counts and declarations without altering the global directory', () => {
  const entries = [
    { name: 'reviewer', kind: 'human', total: 12, projects: [
      { key: 'a', kind: 'agent', total: 2, description: 'Review A' }, { key: 'b', total: 10 },
    ] },
    { name: 'planner', total: 3, projects: [{ key: 'b', total: 3 }] },
    { name: 'owner', total: 0, projects: [{ key: 'a', total: 0 }] },
  ];
  const original = JSON.stringify(entries);
  assert.deepEqual(projectRoles(entries, 'a'), [
    { name: 'reviewer', kind: 'agent', total: 2, projects: [entries[0].projects[0]] },
    { name: 'owner', kind: undefined, total: 0, projects: [entries[2].projects[0]] },
  ]);
  assert.deepEqual(projectRoles(entries, 'missing'), []);
  assert.equal(projectRoles(entries, null), entries);
  assert.equal(JSON.stringify(entries), original);
});

test('project queue scoping keeps its failures and excludes other projects attention and work', () => {
  const view = { projects: [
    { key: 'a', label: 'A', error: 'not loaded' },
    { key: 'b', queue: queue([group('task', 'review', [{ id: 'TASK-0001' }])], [{ id: 'TASK-0002' }]) },
  ] };
  const scoped = scopeRoleView(view, 'a');
  assert.deepEqual(scoped.projects, [view.projects[0]]);
  const model = groupRoleQueues(scoped);
  assert.equal(model.projects[0].error, 'not loaded');
  assert.deepEqual(model.attention, []);
  assert.equal(model.empty, false);
  assert.equal(scopeRoleView(view, null), view);
  assert.equal(scopeRoleView(null, 'a'), null);
  assert.equal(view.projects.length, 2);
});

const group = (type, status, items = [], extra = {}) => ({
  type, status, owners: ['reviewer'], owned: true, total: items.length,
  items, blocked: [], orphaned: [], ...extra,
});
const queue = (groups = [], attention = [], project_problems = []) => ({
  schema: 'opys.queue/1', role: 'reviewer', groups, attention, project_problems,
});

test('queue_groups_keep_engine_order_and_attention_first', () => {
  const ranked = [{ id: 'TASK-0020', position: 1, priority: 30, effective_priority: 1 },
    { id: 'TASK-0001', position: 2, priority: 2, effective_priority: 2 }];
  const view = { projects: [
    { key: 'b', label: 'Second', queue: queue([
      group('task', 'review', ranked), group('task', 'todo'),
      group('task', 'research', [], { owned: false, blocked: [{ id: 'TASK-0004' }] }),
      group('feature', 'approval', [{ id: 'FEAT-0003' }]),
    ], [{ id: 'TASK-0020', reasons: [{ kind: 'stuck', message: 'stuck' }] }]) },
    { key: 'a', label: 'First', queue: queue([group('task', 'review', [{ id: 'TASK-0002' }])]) },
  ] };
  const original = JSON.stringify(view);
  const model = groupRoleQueues(view, [{ key: 'b', corpora: [{ cid: 'branch' }, { cid: 'main', is_primary: true }] }]);
  assert.deepEqual(model.attention.map((entry) => entry.key), ['b']);
  assert.deepEqual(model.projects.map((entry) => entry.key), ['b', 'a']);
  assert.deepEqual(model.projects[0].groups.map((entry) => [entry.type, entry.status]),
    [['task', 'review'], ['task', 'research'], ['feature', 'approval']]);
  assert.deepEqual(model.projects[0].groups[0].items, ranked, 'the engine, not a raw priority sort, ranks rows');
  assert.equal(model.projects[0].cid, 'main');
  assert.equal(model.empty, false);
  assert.equal(JSON.stringify(view), original);
});

test('queue_errors_and_project_problems_are_never_empty_queues', () => {
  const failed = { key: 'failed', label: 'Failed', error: 'inventory not loaded' };
  const model = groupRoleQueues({ projects: [
    failed,
    { key: 'problem', label: 'Problem', queue: queue([], [], ['config needs repair']) },
    { key: 'empty', label: 'Empty', queue: queue([group('task', 'review')]) },
  ] });
  assert.deepEqual(model.projects, [{ ...failed, cid: null }]);
  assert.deepEqual(model.attention[0].problems, ['config needs repair']);
  assert.equal(model.empty, false);
  assert.equal(groupRoleQueues({ projects: [{ key: 'empty', queue: queue() }] }).empty, true);
  assert.equal(groupRoleQueues(null).empty, true);
});

test('queue_override_groups_allow_omitted_status_owners', () => {
  const override = group('task', 'research', [{ id: 'TASK-0004', owners: ['reviewer'], owner_override: true }], { owned: false });
  delete override.owners;
  const model = groupRoleQueues({ projects: [{ key: 'plain', queue: queue([override]) }] });
  assert.deepEqual(model.projects[0].groups[0].owners, []);
  assert.equal(model.projects[0].groups[0].items[0].owner_override, true);
  assert.equal(model.empty, false);
  assert.equal('owners' in override, false, 'display normalization leaves the payload untouched');
});

test('queue_relative_since_formats_elapsed_units_and_unknowns', () => {
  const now = Date.parse('2026-10-19T12:00:00Z');
  for (const [iso, expected] of [
    ['2026-10-19T12:00:00Z', 'just now'], ['2026-10-20T12:00:00Z', 'just now'],
    ['2026-10-19T11:59:00Z', '1 minute'], ['2026-10-19T11:01:00Z', '59 minutes'],
    ['2026-10-19T11:00:00Z', '1 hour'], ['2026-10-18T13:00:00Z', '23 hours'],
    ['2026-10-18T12:00:00Z', '1 day'], ['2026-10-16T12:00:00Z', '3 days'],
    ['', null], [null, null], ['not a timestamp', null],
  ]) assert.equal(relativeSince(iso, now), expected, String(iso));
});

test('queue_refreshes_only_primary_corpora_and_roster_changes', () => {
  const groups = [{ corpora: [{ cid: 'branch' }, { cid: 'main', is_primary: true }] },
    { corpora: [{ cid: 'plain' }] }];
  const event = (entry) => ({ reconnected: false, events: [entry] });
  assert.equal(primaryCorpus(groups[0]).cid, 'main');
  assert.equal(primaryCorpus(groups[1]).cid, 'plain');
  assert.equal(affectsQueues(event({ event: 'corpus-reloaded', cid: 'main' }), groups), true);
  assert.equal(affectsQueues(event({ event: 'corpus-reloaded', cid: 'plain' }), groups), true);
  assert.equal(affectsQueues(event({ event: 'corpus-reloaded', cid: 'branch' }), groups), false);
  assert.equal(affectsQueues(event({ event: 'refs-changed', project: 'repo' }), groups), false);
  assert.equal(affectsQueues(event({ event: 'corpus-added', cid: 'new' }), groups), true);
  assert.equal(affectsQueues(event({ event: 'corpus-removed', cid: 'main' }), groups), true);
  assert.equal(affectsQueues({ reconnected: true, events: [] }, groups), true);
});

test('claimed_only_queues_remain_visible_with_ref_holder_and_notices', () => {
  const claimed = { id: 'TASK-0001', ref: 'spec/TASK-0001', claim: { holder: 'worker-7', since: null, age_seconds: null } };
  const payload = { ...queue([group('task', 'approval', [], { in_progress: [claimed] })]), notices: ['One branch is unreadable'] };
  const model = groupRoleQueues({ projects: [{ key: 'repo', queue: payload }] });
  assert.equal(model.empty, false);
  assert.equal(model.projects[0].groups[0].total, 0);
  assert.deepEqual(model.projects[0].groups[0].in_progress, [claimed]);
  assert.deepEqual(model.projects[0].notices, payload.notices);
  const groups = [{ key: 'repo', corpora: [{ cid: 'main', is_primary: true }] }];
  assert.equal(affectsQueues({ events: [{ event: 'refs-changed', project: 'repo' }] }, groups), true);
  assert.equal(affectsQueues({ events: [{ event: 'refs-changed', project: 'other' }] }, groups), false);
});

test('role selection prefers explicit and remembered choices, then the first human without name inference', () => {
  const entries = [
    { name: 'build', kind: 'automation' },
    { name: 'dan', kind: 'agent' },
    { name: 'reviewer', kind: 'human' },
    { name: 'z-owner', kind: 'human' },
  ];
  assert.equal(selectRole(entries, null, null), 'reviewer');
  assert.equal(selectRole(entries, null, 'dan'), 'dan');
  assert.equal(selectRole(entries, 'build', 'dan'), 'build');
  assert.equal(selectRole(entries, null, 'removed'), 'reviewer');
  assert.equal(selectRole([{ name: 'human' }, { name: 'dan' }], null, null), 'human');
  assert.equal(selectRole([{ name: 'bot', kind: 'agent' }], null, null), 'bot');
  assert.equal(selectRole([], null, null), null);
});

test('role option groups put all humans first and preserve name order within each kind', () => {
  const entries = [
    { name: 'a-script', kind: 'automation' },
    { name: 'b-agent', kind: 'agent' },
    { name: 'c-human', kind: 'human' },
    { name: 'd-legacy' },
    { name: 'e-human', kind: 'human' },
  ];
  const groups = groupRoles(entries);
  assert.deepEqual(groups.map((group) => group.label), ['Human roles', 'Agent roles', 'Automation roles', 'Unspecified roles']);
  assert.deepEqual(groups[0].entries.map((entry) => entry.name), ['c-human', 'e-human']);
  assert.deepEqual(groupRoles([]), []);
  assert.deepEqual(groupRoles([{ name: 'reviewer', kind: 'human' }]).map((group) => group.label), ['Human roles']);
});
