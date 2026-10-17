// Laying a status graph out left to right.
//
// The node hands the browser a graph: a node per distinct status, an edge
// wherever some type's `transitions` map declares the move. Turning that into
// coordinates is the one piece of real logic in the flow view, so it lives here
// — plain ESM, no Svelte, no DOM, no clock — where `node --test` can hold it to
// its invariants without a browser.
//
// Three properties are what the view is drawn against, and all three are
// tested:
//
//  - **A node is never left of its source.** For every edge that is *not* a
//    back edge, `layer(to) > layer(from)`. The escape clause is not a weakness,
//    it is the point: the shipped config is full of returns (`in-progress →
//    todo`, `partial → planned`, `implemented → partial`, `wontfix → planned`),
//    and a layered drawing of a cyclic graph is exactly the set of edges that
//    go forwards plus the set that comes back. Marking which is which is how
//    the renderer draws the second kind differently.
//  - **Components do not overlap.** Two weakly-connected components occupy
//    disjoint bands of rows, with a blank row between them, so the shipped
//    config's two graphs — work items and features — read as two pictures
//    rather than one tangle (this repository's own config adds `adr` and draws
//    three).
//  - **A cycle terminates.** `draft ↔ live` is a two-cycle in the default test
//    config and this repo's own config has half a dozen; a longest-path
//    relaxation that did not break them would not return at all.
//
// Everything is a pure function of its arguments and deterministic: the same
// input twice gives byte-identical output, because every tie is broken by the
// order the node sent — which is the config's declaration order, the same one
// `opys list --sort` and `opys next` rank by.

/**
 * @typedef {{status: string, initial?: boolean}} GraphNode
 * @typedef {{from: string, to: string}} GraphEdge
 * @typedef {{status: string, layer: number, row: number, component: number}} PlacedNode
 * @typedef {{from: string, to: string, back: boolean}} PlacedEdge
 * @typedef {{nodes: PlacedNode[], edges: PlacedEdge[], layers: number, rows: number}} Layout
 */

/** Draw unrestricted workflows honestly, without inventing a preferred sequence. */
export function workflow(view, typeName) {
  const types = (view?.types ?? []).filter((type) => !typeName || type.name === typeName);
  const statuses = new Set(types.flatMap((type) => type.statuses));
  const nodes = (view?.nodes ?? []).filter((node) => !typeName || statuses.has(node.status));
  const edges = new Map();
  for (const edge of view?.edges ?? []) {
    const owners = edge.types.filter((name) => types.some((type) => type.name === name));
    if (owners.length) edges.set(JSON.stringify([edge.from, edge.to]), { ...edge, implicit: false });
  }
  for (const type of types.filter((type) => !type.has_graph)) {
    for (const from of type.statuses) {
      // Terminal work items are deleted by close; they cannot leave that state.
      if (type.terminal_statuses.includes(from)) continue;
      for (const to of type.statuses) {
        if (from === to) continue;
        const key = JSON.stringify([from, to]);
        if (!edges.has(key)) edges.set(key, { from, to, implicit: true });
      }
    }
  }
  return {
    nodes: nodes.map((node) => ({ ...node,
      initial: types.some((type) => type.default_status === node.status),
      terminal: types.some((type) => type.terminal_statuses.includes(node.status)),
    })),
    edges: [...edges.values()],
    unrestricted: types.some((type) => !type.has_graph),
  };
}

/**
 * Place a status graph on a grid of layers (columns) and rows.
 *
 * @param {GraphNode[]} nodes in the order the node sent them
 * @param {GraphEdge[]} edges
 * @returns {Layout}
 */
export function layout(nodes, edges) {
  const order = [];
  const seen = new Set();
  for (const node of nodes ?? []) {
    const status = node?.status;
    if (typeof status !== 'string' || seen.has(status)) continue;
    seen.add(status);
    order.push(status);
  }
  const initial = new Set(
    (nodes ?? []).filter((node) => node?.initial === true).map((node) => node.status),
  );

  // Edges, in the order given, with the two kinds of return already marked:
  //
  //  - a self-loop, which no layering can satisfy;
  //  - an edge *into* an initial status, which is what literally delivers
  //    "initial statuses in layer 0". In the shipped config this alone accounts
  //    for `in-progress → todo`, `wontfix → planned` and `archived → planned`.
  //
  // An edge naming a status that is not a node is dropped rather than invented:
  // the node's payload is built from the same config the edges come from, so
  // this only happens for a config `verify` already reports.
  /** @type {PlacedEdge[]} */
  const placedEdges = [];
  for (const edge of edges ?? []) {
    if (!seen.has(edge?.from) || !seen.has(edge?.to)) continue;
    placedEdges.push({
      from: edge.from,
      to: edge.to,
      back: edge.from === edge.to || initial.has(edge.to),
    });
  }

  /** status → its outgoing edges, in input order. */
  const out = new Map(order.map((status) => [status, []]));
  for (const edge of placedEdges) out.get(edge.from).push(edge);

  const component = components(order, placedEdges);
  breakCycles(order, out, initial, placedEdges);
  const layer = layerNodes(order, out, placedEdges);
  const row = rows(order, component, layer);

  return {
    nodes: order.map((status) => ({
      status,
      layer: layer.get(status),
      row: row.get(status),
      component: component.get(status),
    })),
    edges: placedEdges,
    layers: order.length === 0 ? 0 : Math.max(...order.map((s) => layer.get(s))) + 1,
    rows: order.length === 0 ? 0 : Math.max(...order.map((s) => row.get(s))) + 1,
  };
}

/**
 * Weakly-connected components, numbered by their first node in input order.
 *
 * Undirected, and over *every* edge including the ones already marked back: two
 * statuses joined only by a return are still one picture, and splitting them
 * would put `todo` and `in-progress` in different bands.
 *
 * @returns {Map<string, number>}
 */
function components(order, edges) {
  const neighbours = new Map(order.map((status) => [status, []]));
  for (const edge of edges) {
    neighbours.get(edge.from).push(edge.to);
    neighbours.get(edge.to).push(edge.from);
  }
  const of = new Map();
  let next = 0;
  for (const root of order) {
    if (of.has(root)) continue;
    const id = next++;
    const queue = [root];
    of.set(root, id);
    while (queue.length > 0) {
      const at = queue.shift();
      for (const to of neighbours.get(at)) {
        if (of.has(to)) continue;
        of.set(to, id);
        queue.push(to);
      }
    }
  }
  return of;
}

/**
 * Mark every remaining edge that closes a cycle, by depth-first search.
 *
 * Iterative, with an explicit stack: a status graph is small, but a recursive
 * walk over a hostile config is the kind of thing that turns a rendering bug
 * into a blank page. An edge reaching a node that is *on the current stack* is
 * the definition of a back edge, and the first traversal wins — which is the
 * spec's "cycles broken by keeping the first assignment".
 *
 * Roots are tried in a deliberate order: the initial statuses first (a graph
 * should be read from where documents enter it), then the nodes nothing points
 * at, then whatever is still unvisited — a component that is one pure cycle has
 * neither of the first two, and would otherwise never be walked at all.
 */
function breakCycles(order, out, initial, edges) {
  const indegree = new Map(order.map((status) => [status, 0]));
  for (const edge of edges) {
    if (edge.back) continue;
    indegree.set(edge.to, indegree.get(edge.to) + 1);
  }
  const roots = [
    ...order.filter((status) => initial.has(status)),
    ...order.filter((status) => indegree.get(status) === 0),
    ...order,
  ];

  /** 0 unvisited, 1 on the stack, 2 finished. */
  const state = new Map(order.map((status) => [status, 0]));
  for (const root of roots) {
    if (state.get(root) !== 0) continue;
    state.set(root, 1);
    const stack = [{ at: root, i: 0 }];
    while (stack.length > 0) {
      const frame = stack[stack.length - 1];
      const outgoing = out.get(frame.at);
      if (frame.i >= outgoing.length) {
        state.set(frame.at, 2);
        stack.pop();
        continue;
      }
      const edge = outgoing[frame.i++];
      if (edge.back) continue;
      const to = state.get(edge.to);
      if (to === 1) {
        edge.back = true;
      } else if (to === 0) {
        state.set(edge.to, 1);
        stack.push({ at: edge.to, i: 0 });
      }
    }
  }
}

/**
 * Longest-path layering over the edges that remain, by Kahn's algorithm.
 *
 * `layer(v)` is one past its **deepest** source, not its shallowest: a breadth
 * -first answer would put the far end of a diamond beside the near one and draw
 * an edge going nowhere. Sources sit at 0.
 *
 * The final sweep is defensive: `breakCycles` leaves an acyclic graph, so
 * nothing should be left in the queue's shadow — but if anything ever were, its
 * inbound edges become back edges rather than silently breaking the invariant
 * every caller draws against.
 *
 * @returns {Map<string, number>}
 */
function layerNodes(order, out, edges) {
  const indegree = new Map(order.map((status) => [status, 0]));
  for (const edge of edges) {
    if (edge.back) continue;
    indegree.set(edge.to, indegree.get(edge.to) + 1);
  }
  const layer = new Map(order.map((status) => [status, 0]));
  const queue = order.filter((status) => indegree.get(status) === 0);
  const settled = new Set(queue);
  while (queue.length > 0) {
    const at = queue.shift();
    for (const edge of out.get(at)) {
      if (edge.back) continue;
      layer.set(edge.to, Math.max(layer.get(edge.to), layer.get(at) + 1));
      indegree.set(edge.to, indegree.get(edge.to) - 1);
      if (indegree.get(edge.to) === 0) {
        settled.add(edge.to);
        queue.push(edge.to);
      }
    }
  }
  for (const edge of edges) {
    if (edge.back) continue;
    if (!settled.has(edge.to) || layer.get(edge.to) <= layer.get(edge.from)) edge.back = true;
  }
  return layer;
}

/**
 * A row per node: its index within its layer, offset so each component owns a
 * band of rows of its own with one blank row between bands.
 *
 * @returns {Map<string, number>}
 */
function rows(order, component, layer) {
  const ids = [...new Set(order.map((status) => component.get(status)))];
  const row = new Map();
  let offset = 0;
  for (const id of ids) {
    /** layer → how many nodes of this component are already on it. */
    const filled = new Map();
    let height = 0;
    for (const status of order) {
      if (component.get(status) !== id) continue;
      const at = layer.get(status);
      const index = filled.get(at) ?? 0;
      filled.set(at, index + 1);
      row.set(status, offset + index);
      height = Math.max(height, index + 1);
    }
    offset += height + 1;
  }
  return row;
}
