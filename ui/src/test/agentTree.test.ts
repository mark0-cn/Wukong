import { describe, expect, it } from 'vitest';
import { agentPath, buildAgentTree, visibleAgentRows } from '../components/agentTree';
import { agents } from './fixtures';

describe('agent hierarchy projection', () => {
  it('uses parent identity rather than role or supplied depth and counts only descendants', () => {
    const tree = buildAgentTree(agents.map(agent => ({ ...agent, depth: 99 })));
    expect(tree.children.get('root')).toEqual(['analyst', 'verifier']);
    expect(tree.counts.get('root')).toEqual({ total: 2, running: 2, failed: 0, waiting: 0, completed: 0 });
    expect(visibleAgentRows(tree, new Set()).map(row => [row.id, row.depth])).toEqual([['root', 0], ['analyst', 1], ['verifier', 1]]);
    expect(visibleAgentRows(tree, new Set(['root'])).map(row => row.id)).toEqual(['root']);
    expect(agentPath(tree, 'verifier')).toEqual(['root', 'verifier']);
  });

  it('retains disconnected cycles and missing parents even alongside valid roots', () => {
    const tree = buildAgentTree([...agents,
      { ...agents[1], id: 'cycle-a', parent_id: 'cycle-b' },
      { ...agents[1], id: 'cycle-b', parent_id: 'cycle-a' },
      { ...agents[1], id: 'orphan', parent_id: 'missing' },
      { ...agents[1], id: 'self', parent_id: 'self' },
    ]);
    expect(new Set(visibleAgentRows(tree, new Set()).map(row => row.id)).size).toBe(7);
    expect(tree.issues.has('orphan')).toBe(true);
    expect(tree.issues.has('self')).toBe(true);
    expect(agentPath(tree, 'cycle-b').length).toBeLessThanOrEqual(2);
  });

  it('aggregates failures independently of collapse and bounds the visible depth', () => {
    const chain = Array.from({ length: 1000 }, (_, index) => ({ ...agents[1], id: `node-${index}`, parent_id: index ? `node-${index - 1}` : null, status: index === 999 ? 'failed' as const : 'completed' as const }));
    const tree = buildAgentTree(chain);
    expect(tree.counts.get('node-0')).toEqual({ total: 999, running: 0, waiting: 0, failed: 1, completed: 998 });
    const rows = visibleAgentRows(tree, new Set());
    expect(rows).toHaveLength(4);
    expect(rows.at(-1)?.limited).toBe(true);
    expect(visibleAgentRows(tree, new Set(), 'node-3')[0].id).toBe('node-3');
    expect(agentPath(tree, 'node-999')).toHaveLength(1000);
  });
});
