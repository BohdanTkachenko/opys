// Semantic graph compression and ELK layout for the lifecycle inspector.
// Wildcard rules are represented once; they are not synthetic document states.
export function buildWorkflowMap(view, typeName, documents = []) {
  const type = view?.types.find((candidate) => candidate.name === typeName);
  if (!type) return { nodes: [], edges: [], type: null };
  const statuses = [...new Set([...type.statuses, ...documents.filter((doc) => doc.type === typeName).map((doc) => doc.status)])];
  const nodes = statuses.map((status) => ({
    id: `state-${encodeURIComponent(status)}`, status, kind: 'status', width: 172, height: 76,
    initial: status === type.default_status,
    terminal: type.terminal_statuses.includes(status),
    declared: type.statuses.includes(status),
  }));
  const byStatus = new Map(nodes.map((node) => [node.status, node.id]));
  let edges;
  if (type.has_graph) {
    edges = view.edges.filter((edge) => edge.types.includes(typeName))
      .filter((edge) => byStatus.has(edge.from) && byStatus.has(edge.to))
      .map((edge, index) => ({ id: `move-${index}`, from: byStatus.get(edge.from), to: byStatus.get(edge.to), closing: type.terminal_statuses.includes(edge.to), wildcard: false }));
  } else {
    // There is no useful wildcard rule if every state is terminal.
    const active = nodes.filter((node) => node.declared && !node.terminal);
    if (active.length) {
      nodes.unshift({ id: 'any-active', kind: 'rule', width: 196, height: 84 });
      edges = nodes.filter((node) => node.kind === 'status' && node.declared && (active.length > 1 || node.status !== active[0].status))
        .map((node, index) => ({ id: `rule-${index}`, from: 'any-active', to: node.id, closing: node.terminal, wildcard: true }));
    } else edges = [];
  }
  return { type, nodes, edges };
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

let engine;
export async function layoutWorkflowMap(model) {
  if (!model.nodes.length) return { nodes: [], edges: [], width: 1, height: 1 };
  // Loaded only when the lifecycle is opened; everything is bundled locally.
  const { default: ELK } = await import('elkjs/lib/elk.bundled.js');
  engine ??= new ELK();
  const result = await engine.layout({
    id: 'workflow',
    layoutOptions: {
      'elk.algorithm': 'layered', 'elk.direction': model.type?.has_graph ? 'DOWN' : 'RIGHT',
      'elk.edgeRouting': 'ORTHOGONAL', 'elk.spacing.nodeNode': '24',
      'elk.layered.spacing.nodeNodeBetweenLayers': '88',
      'elk.layered.spacing.edgeNodeBetweenLayers': '24',
      'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
      'elk.layered.cycleBreaking.strategy': 'MODEL_ORDER',
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
