import type { Agent } from '../types';

export interface Descendants { total: number; running: number; waiting: number; failed: number; completed: number }
export interface AgentTree {
  nodes: Map<string, Agent>;
  parents: Map<string, string | null>;
  children: Map<string | null, string[]>;
  issues: Set<string>;
  counts: Map<string, Descendants>;
}

/** Presentation only: parent_id is the hierarchy, never role, scope or depth. */
export function buildAgentTree(agents: Agent[]): AgentTree {
  const nodes = new Map(agents.map(agent => [agent.id, agent]));
  const parents = new Map<string, string | null>();
  const issues = new Set<string>();
  for (const agent of nodes.values()) {
    const valid = agent.parent_id !== agent.id && (!agent.parent_id || nodes.has(agent.parent_id));
    parents.set(agent.id, valid ? agent.parent_id : null);
    if (!valid) issues.add(agent.id);
  }
  // Break only the display edge of a malformed cycle. Keep every node inspectable.
  const checked = new Set<string>();
  for (const id of nodes.keys()) {
    const path = new Set<string>();
    let current: string | null = id;
    while (current && !checked.has(current)) {
      if (path.has(current)) { parents.set(current, null); issues.add(current); break; }
      path.add(current); current = parents.get(current) ?? null;
    }
    path.forEach(value => checked.add(value));
  }
  const children = new Map<string | null, string[]>();
  for (const [id, parent] of parents) {
    const siblings = children.get(parent) ?? [];
    siblings.push(id); children.set(parent, siblings);
  }
  const order: string[] = [], stack = [...(children.get(null) ?? [])];
  while (stack.length) { const id = stack.pop()!; order.push(id); stack.push(...(children.get(id) ?? [])); }
  const counts = new Map<string, Descendants>();
  for (const id of order.reverse()) {
    const count: Descendants = { total: 0, running: 0, waiting: 0, failed: 0, completed: 0 };
    for (const child of children.get(id) ?? []) {
      const nested = counts.get(child)!;
      for (const key of Object.keys(count) as (keyof Descendants)[]) count[key] += nested[key];
      count.total++;
      const state = nodes.get(child)!.status;
      if (state === 'running') count.running++;
      else if (['failed', 'crashed'].includes(state)) count.failed++;
      else if (state === 'completed') count.completed++;
      else if (['waiting', 'queued', 'budget_paused'].includes(state)) count.waiting++;
    }
    counts.set(id, count);
  }
  return { nodes, parents, children, issues, counts };
}

export function agentPath(tree: AgentTree, id?: string): string[] {
  const path: string[] = [];
  let current = id;
  while (current && tree.nodes.has(current)) { path.push(current); current = tree.parents.get(current) ?? undefined; }
  return path.reverse();
}

export interface TreeRow {
  id: string; depth: number; rails: boolean[]; last: boolean; position: number; size: number; limited: boolean;
}
export function visibleAgentRows(tree: AgentTree, collapsed: Set<string>, focus?: string): TreeRow[] {
  const roots = focus && tree.nodes.has(focus) ? [focus] : tree.children.get(null) ?? [];
  const rows: TreeRow[] = [];
  const walk = (siblings: string[], rails: boolean[]) => siblings.forEach((id, index) => {
    const last = index === siblings.length - 1;
    const children = tree.children.get(id) ?? [];
    const depth = rails.length;
    const limited = depth >= 3 && children.length > 0;
    rows.push({ id, depth, rails, last, position: index + 1, size: siblings.length, limited });
    if (!collapsed.has(id) && !limited) walk(children, [...rails, !last]);
  });
  walk(roots, []);
  return rows;
}
