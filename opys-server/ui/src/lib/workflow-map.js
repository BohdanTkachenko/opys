// The complete permitted-transition graph. Shared status names and transitions
// merge across types; every edge retains its owners and closing semantics.
export function buildWorkflowMap(view, typeName = null, documents = []) {
  const types = (view?.types ?? []).filter((type) => !typeName || type.name === typeName);
  if (!types.length) return { nodes: [], edges: [], types: [] };
  const statuses = [...new Set([...types.flatMap((type) => type.statuses), ...documents.filter((doc) => !typeName || doc.type === typeName).map((doc) => doc.status)])];
  const nodes = statuses.map((status) => ({
    id: `state-${encodeURIComponent(status)}`, status, kind: 'status', width: 172, height: 76,
    types: types.filter((type) => type.statuses.includes(status)).map((type) => type.name),
    initial: types.some((type) => status === type.default_status),
    terminal: types.some((type) => type.statuses.includes(status)) && types.filter((type) => type.statuses.includes(status)).every((type) => type.terminal_statuses.includes(status)),
    declared: types.some((type) => type.statuses.includes(status)),
  }));
  const byStatus = new Map(nodes.map((node) => [node.status, node.id]));
  const edges = new Map();
  function add(type, from, to, unrestricted) {
    if (!byStatus.has(from) || !byStatus.has(to)) return;
    const id = JSON.stringify([from, to]);
    const edge = edges.get(id) ?? { id, from: byStatus.get(from), to: byStatus.get(to), types: [], closingTypes: [], unrestricted: true };
    if (!edge.types.includes(type.name)) edge.types.push(type.name);
    if (type.terminal_statuses.includes(to) && !edge.closingTypes.includes(type.name)) edge.closingTypes.push(type.name);
    edge.unrestricted &&= unrestricted;
    edges.set(id, edge);
  }
  for (const type of types) {
    if (type.has_graph) {
      for (const edge of view.edges ?? []) if (edge.types.includes(type.name)) add(type, edge.from, edge.to, false);
    } else {
      for (const from of type.statuses) {
        if (type.terminal_statuses.includes(from)) continue;
        for (const to of type.statuses) if (from !== to) add(type, from, to, true);
      }
    }
  }
  return { types, nodes, edges: [...edges.values()].map((edge) => ({ ...edge, closing: edge.closingTypes.length === edge.types.length })), direction: typeName ? 'DOWN' : 'RIGHT' };
}

export function roundedRoute(points, radius = 8) {
  if (points.length < 2) return '';
  let path = `M ${points[0].x} ${points[0].y}`;
  for (let i = 1; i < points.length - 1; i++) {
    const previous = points[i - 1], corner = points[i], next = points[i + 1];
    const incoming = Math.hypot(corner.x - previous.x, corner.y - previous.y);
    const outgoing = Math.hypot(next.x - corner.x, next.y - corner.y);
    if (!incoming || !outgoing) continue;
    const r = Math.min(radius, incoming / 2, outgoing / 2);
    const a = { x: corner.x + (previous.x - corner.x) * r / incoming, y: corner.y + (previous.y - corner.y) * r / incoming };
    const b = { x: corner.x + (next.x - corner.x) * r / outgoing, y: corner.y + (next.y - corner.y) * r / outgoing };
    path += ` L ${a.x} ${a.y} Q ${corner.x} ${corner.y} ${b.x} ${b.y}`;
  }
  const last = points.at(-1);
  return `${path} L ${last.x} ${last.y}`;
}

/** Fit the complete graph in the viewport, including its drawing margin. */
export function fitWorkflowMap(width, height, viewportWidth, viewportHeight) {
  return Math.min(1.15, Math.max(1, viewportWidth - 40) / Math.max(1, width), Math.max(1, viewportHeight - 40) / Math.max(1, height));
}

let engine;
export async function layoutWorkflowMap(model) {
  if (!model.nodes.length) return { nodes: [], edges: [], width: 1, height: 1 };
  // Loaded only when the lifecycle is opened; everything is bundled locally.
  const { default: ELK } = await import('elkjs/lib/elk.bundled.js');
  engine ??= new ELK();
  const result = await engine.layout({
    id: 'workflow',
    layoutOptions: {
      'elk.algorithm': 'layered', 'elk.direction': model.direction ?? 'RIGHT',
      'elk.edgeRouting': 'ORTHOGONAL', 'elk.spacing.nodeNode': '24',
      'elk.layered.spacing.nodeNodeBetweenLayers': '88',
      'elk.layered.spacing.edgeNodeBetweenLayers': '24',
      'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
      'elk.layered.cycleBreaking.strategy': 'MODEL_ORDER',
      'elk.layered.mergeEdges': 'false',
      'elk.layered.crossingMinimization.forceNodeModelOrder': 'true',
      'elk.padding': '[top=24,left=24,bottom=24,right=24]',
    },
    children: model.nodes.map(({ id, width, height }) => ({ id, width, height })),
    edges: model.edges.map(({ id, from, to }) => ({ id, sources: [from], targets: [to] })),
  });
  return {
    width: result.width, height: result.height,
    nodes: result.children.map((node) => ({ ...model.nodes.find((item) => item.id === node.id), x: node.x, y: node.y })),
    edges: result.edges.map((edge) => ({ ...model.edges.find((item) => item.id === edge.id),
      paths: (edge.sections ?? []).map((section) => roundedRoute([section.startPoint, ...(section.bendPoints ?? []), section.endPoint])),
    })),
  };
}
