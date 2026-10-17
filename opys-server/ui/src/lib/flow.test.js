// The layout's invariants, run by Node's built-in test runner:
//
//     cd opys-server/ui && npm test
//
// No test framework and no new dependency: `node --test` ships with the Node 22
// the devShell and CI already pin, and adding vitest would change
// `package-lock.json` and with it the `npmDepsHash` the Nix build pins. Nothing
// imports this file, so Vite never puts it in the bundle.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { layout } from './flow.js';

/** The three types of the API test's fixture config, as the node sends them. */
const FIXTURE_NODES = [
  { status: 'open', initial: true },
  { status: 'closed', initial: false },
  { status: 'todo', initial: true },
  { status: 'doing', initial: false },
  { status: 'done', initial: false },
  { status: 'draft', initial: true },
  { status: 'live', initial: false },
];
const FIXTURE_EDGES = [
  { from: 'todo', to: 'doing' },
  { from: 'doing', to: 'todo' },
  { from: 'doing', to: 'done' },
  { from: 'draft', to: 'live' },
  { from: 'live', to: 'draft' },
];

/** status → its placement, for readable assertions. */
function placed(result) {
  return new Map(result.nodes.map((node) => [node.status, node]));
}

test('a node is never left of its source, except on a marked back edge', () => {
  const result = layout(FIXTURE_NODES, FIXTURE_EDGES);
  const at = placed(result);
  for (const edge of result.edges) {
    if (edge.back) continue;
    assert.ok(
      at.get(edge.to).layer > at.get(edge.from).layer,
      `${edge.from} → ${edge.to} goes backwards without being marked`,
    );
  }
});

test('an edge into an initial status is a back edge', () => {
  const result = layout(FIXTURE_NODES, FIXTURE_EDGES);
  const back = result.edges.filter((edge) => edge.back).map((edge) => `${edge.from}->${edge.to}`);
  assert.deepEqual(back.sort(), ['doing->todo', 'live->draft']);
  const at = placed(result);
  assert.equal(at.get('todo').layer, 0);
  assert.equal(at.get('draft').layer, 0);
  assert.equal(at.get('open').layer, 0);
});

test('no two nodes share a cell, and components occupy disjoint row bands', () => {
  const result = layout(FIXTURE_NODES, FIXTURE_EDGES);
  const cells = new Set();
  for (const node of result.nodes) {
    const cell = `${node.layer}/${node.row}`;
    assert.ok(!cells.has(cell), `two nodes at ${cell}`);
    cells.add(cell);
  }

  /** component → the rows it uses. */
  const bands = new Map();
  for (const node of result.nodes) {
    if (!bands.has(node.component)) bands.set(node.component, new Set());
    bands.get(node.component).add(node.row);
  }
  // Four, not three: `note` declares no transitions, so `open` and `closed`
  // are two isolated nodes rather than one two-node picture. That is honest —
  // any-to-any means the config never said `open → closed` — and it is why the
  // renderer must not read "no edges" as "one component".
  assert.equal(bands.size, 4);
  const used = new Set();
  for (const rows of bands.values()) {
    for (const row of rows) {
      assert.ok(!used.has(row), `row ${row} is shared between components`);
      used.add(row);
    }
  }
});

test('a pure cycle terminates and every node still gets a finite layer', () => {
  // No initial status anywhere, so the pre-marking pass cannot help: the DFS is
  // the only thing that breaks this. A hang is the failure.
  const nodes = [{ status: 'a' }, { status: 'b' }, { status: 'c' }];
  const edges = [
    { from: 'a', to: 'b' },
    { from: 'b', to: 'c' },
    { from: 'c', to: 'a' },
  ];
  const result = layout(nodes, edges);
  assert.deepEqual(
    result.nodes.map((node) => node.layer),
    [0, 1, 2],
  );
  assert.deepEqual(
    result.edges.map((edge) => edge.back),
    [false, false, true],
  );
  assert.equal(result.layers, 3);
});

test('a self-loop is a back edge and changes no layer', () => {
  const nodes = [{ status: 'a', initial: true }, { status: 'b' }];
  const edges = [
    { from: 'a', to: 'b' },
    { from: 'b', to: 'b' },
  ];
  const result = layout(nodes, edges);
  assert.deepEqual(
    result.edges.map((edge) => edge.back),
    [false, true],
  );
  assert.equal(placed(result).get('b').layer, 1);
});

test('a target sits one layer past its deepest source, not its shallowest', () => {
  // The diamond: shortest-path layering would put `c` at 1, beside `b`, and
  // draw `b → c` as an edge with nowhere to go.
  const nodes = [{ status: 'a', initial: true }, { status: 'b' }, { status: 'c' }];
  const edges = [
    { from: 'a', to: 'b' },
    { from: 'a', to: 'c' },
    { from: 'b', to: 'c' },
  ];
  const at = placed(layout(nodes, edges));
  assert.equal(at.get('a').layer, 0);
  assert.equal(at.get('b').layer, 1);
  assert.equal(at.get('c').layer, 2);
});

test('a type that declares no transitions is N nodes, no edges, all in layer 0', () => {
  const nodes = [{ status: 'open', initial: true }, { status: 'closed' }];
  const result = layout(nodes, []);
  assert.deepEqual(result.edges, []);
  assert.deepEqual(
    result.nodes.map((node) => [node.layer, node.row, node.component]),
    [
      [0, 0, 0],
      [0, 2, 1],
    ],
    'two isolated statuses are two components, one blank row apart',
  );
});

test('an edge naming a status that is not a node is dropped', () => {
  const result = layout([{ status: 'a', initial: true }], [{ from: 'a', to: 'ghost' }]);
  assert.deepEqual(result.edges, []);
  assert.equal(result.nodes.length, 1);
});

test('the same input twice gives the same output', () => {
  const first = layout(FIXTURE_NODES, FIXTURE_EDGES);
  const second = layout(FIXTURE_NODES, FIXTURE_EDGES);
  assert.deepEqual(first, second);
  // And the inputs are untouched — the caller keeps a `$derived` over them.
  assert.deepEqual(FIXTURE_EDGES[1], { from: 'doing', to: 'todo' });
});

test('an empty graph is an empty layout rather than a crash', () => {
  assert.deepEqual(layout([], []), { nodes: [], edges: [], layers: 0, rows: 0 });
  assert.deepEqual(layout(undefined, undefined), { nodes: [], edges: [], layers: 0, rows: 0 });
});

test("this repo's own lifecycle draws as three readable components", () => {
  // This repository's own `opys.toml`, which is what the algorithm has to
  // survive: cycles across three disconnected graphs — work items, features,
  // decisions. (The *shipped* config, the one `opys init` writes, has no `adr`
  // type and so draws two.)
  //
  // Transcribed by hand from `opys.toml` — every status of every type in
  // declaration order, then every `transitions` target in declaration order,
  // which is the order the node's payload sends them. It is a pin, so when that
  // config's `transitions` change, re-transcribe it here: `[types.feature]`
  // statuses/transitions, `[types.adr]`, `[types.task]` (`bug` and `chore`
  // declare the same graph and merge into it).
  const nodes = [
    { status: 'planned', initial: true },
    { status: 'partial' },
    { status: 'implemented' },
    { status: 'wontfix' },
    { status: 'archived' },
    { status: 'proposed', initial: true },
    { status: 'accepted' },
    { status: 'superseded' },
    { status: 'todo', initial: true },
    { status: 'in-progress' },
    { status: 'review' },
    { status: 'user-review' },
    { status: 'done' },
  ];
  const edges = [
    { from: 'planned', to: 'partial' },
    { from: 'planned', to: 'implemented' },
    { from: 'planned', to: 'wontfix' },
    { from: 'partial', to: 'planned' },
    { from: 'partial', to: 'implemented' },
    { from: 'partial', to: 'wontfix' },
    { from: 'implemented', to: 'partial' },
    { from: 'implemented', to: 'archived' },
    { from: 'wontfix', to: 'planned' },
    { from: 'archived', to: 'planned' },
    { from: 'proposed', to: 'accepted' },
    { from: 'proposed', to: 'superseded' },
    { from: 'accepted', to: 'superseded' },
    { from: 'todo', to: 'in-progress' },
    { from: 'in-progress', to: 'review' },
    { from: 'in-progress', to: 'todo' },
    { from: 'review', to: 'done' },
    { from: 'review', to: 'in-progress' },
    { from: 'review', to: 'user-review' },
    { from: 'user-review', to: 'done' },
    { from: 'user-review', to: 'in-progress' },
  ];
  const result = layout(nodes, edges);
  const at = placed(result);
  assert.equal(new Set(result.nodes.map((node) => node.component)).size, 3);
  assert.deepEqual(
    ['planned', 'partial', 'wontfix', 'implemented', 'archived'].map((s) => at.get(s).layer),
    // `wontfix` sits one past its *deepest* source: both `planned` and
    // `partial` propose it, so it lands beside `implemented`, not beside
    // `partial`.
    [0, 1, 2, 2, 3],
  );
  assert.deepEqual(
    ['proposed', 'accepted', 'superseded'].map((s) => at.get(s).layer),
    [0, 1, 2],
  );
  assert.deepEqual(
    ['todo', 'in-progress', 'review', 'user-review', 'done'].map((s) => at.get(s).layer),
    [0, 1, 2, 3, 4],
  );
  for (const edge of result.edges) {
    if (edge.back) continue;
    assert.ok(at.get(edge.to).layer > at.get(edge.from).layer, `${edge.from} → ${edge.to}`);
  }
});

test('unrestricted workflows draw allowed arrows without terminal departures', async () => {
  const { workflow } = await import('./flow.js');
  const view = {
    types: [{name: 'task', statuses: ['todo', 'doing', 'done'], default_status: 'todo', terminal_statuses: ['done'], has_graph: false}],
    nodes: ['todo', 'doing', 'done'].map(status => ({status})), edges: [],
  };
  const graph = workflow(view, 'task');
  assert.equal(graph.edges.length, 4);
  assert.ok(graph.edges.every(edge => edge.implicit && edge.from !== 'done' && edge.from !== edge.to));
  assert.ok(graph.edges.some(edge => edge.from === 'doing' && edge.to === 'todo'));
  assert.equal(layout(graph.nodes, graph.edges).edges.length, 4);
});

test('declared workflows keep only their configured arrows', async () => {
  const { workflow } = await import('./flow.js');
  const view = {
    types: [{name: 'task', statuses: ['todo', 'done'], default_status: 'todo', terminal_statuses: ['done'], has_graph: true}],
    nodes: ['todo', 'done', 'accepted'].map(status => ({status})),
    edges: [{from: 'todo', to: 'done', types: ['task']}],
  };
  const graph = workflow(view, 'task');
  assert.equal(graph.nodes.length, 2);
  assert.equal(graph.edges.length, 1);
  assert.equal(graph.edges[0].implicit, false);
  assert.equal(workflow(view, null).nodes.length, 3, 'all types preserves unowned statuses');
});
