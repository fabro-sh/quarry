export interface GraphLayoutNode {
  id: string;
}

export interface GraphLayoutPosition {
  id: string;
  x: number;
  y: number;
}

interface LayoutWorkerLike {
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  postMessage: (message: unknown) => void;
  terminate: () => void;
}

interface LayoutOptions {
  createWorker?: () => LayoutWorkerLike | null;
}

export function circularLayout(nodes: GraphLayoutNode[]): GraphLayoutPosition[] {
  const radius = 1;
  return nodes.map((node, index) => {
    const angle = (index / Math.max(nodes.length, 1)) * Math.PI * 2;
    return {
      id: node.id,
      x: Math.cos(angle) * radius,
      y: Math.sin(angle) * radius,
    };
  });
}

export function layoutGraphNodes(
  nodes: GraphLayoutNode[],
  options: LayoutOptions = {}
): Promise<GraphLayoutPosition[]> {
  const worker = options.createWorker ? options.createWorker() : createDefaultLayoutWorker();
  if (!worker) return Promise.resolve(circularLayout(nodes));
  const layoutWorker = worker;

  return new Promise((resolve) => {
    let settled = false;
    const timeout = window.setTimeout(() => finish(circularLayout(nodes)), 2000);

    function finish(positions: GraphLayoutPosition[]) {
      if (settled) return;
      settled = true;
      window.clearTimeout(timeout);
      layoutWorker.terminate();
      resolve(positions);
    }

    layoutWorker.onmessage = (event) => {
      const positions = parsePositions(event.data);
      finish(positions ?? circularLayout(nodes));
    };
    layoutWorker.onerror = () => finish(circularLayout(nodes));

    try {
      layoutWorker.postMessage({ nodes });
    } catch {
      finish(circularLayout(nodes));
    }
  });
}

function parsePositions(value: unknown): GraphLayoutPosition[] | null {
  if (!Array.isArray(value)) return null;
  const positions: GraphLayoutPosition[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== 'object') return null;
    const position = entry as Record<string, unknown>;
    if (
      typeof position.id !== 'string' ||
      typeof position.x !== 'number' ||
      typeof position.y !== 'number'
    ) {
      return null;
    }
    positions.push({ id: position.id, x: position.x, y: position.y });
  }
  return positions;
}

function createDefaultLayoutWorker(): LayoutWorkerLike | null {
  if (
    typeof Worker === 'undefined' ||
    typeof Blob === 'undefined' ||
    typeof URL === 'undefined' ||
    typeof URL.createObjectURL !== 'function'
  ) {
    return null;
  }

  const blob = new Blob([layoutWorkerScript()], { type: 'text/javascript' });
  const url = URL.createObjectURL(blob);
  const worker = new Worker(url) as LayoutWorkerLike;
  const terminate = worker.terminate.bind(worker);
  worker.terminate = () => {
    terminate();
    URL.revokeObjectURL(url);
  };
  return worker;
}

function layoutWorkerScript() {
  return `
self.onmessage = function(event) {
  var nodes = event.data && Array.isArray(event.data.nodes) ? event.data.nodes : [];
  var radius = 1;
  var positions = nodes.map(function(node, index) {
    var angle = (index / Math.max(nodes.length, 1)) * Math.PI * 2;
    return {
      id: String(node.id),
      x: Math.cos(angle) * radius,
      y: Math.sin(angle) * radius
    };
  });
  self.postMessage(positions);
};
`;
}
