import { t, useLocale } from '../i18n';
import { useMemo, useState } from 'react';
import { Bot, ChevronDown, ChevronRight, GitBranch, LayoutList, PanelRightClose } from 'lucide-react';
import type { Agent } from '../types';
import { compact, labels, Status, terminal } from './Status';

export function Agents({ agents, selected, onSelect, onStop, canStop, onClose }: {
  agents: Agent[]; selected?: string; onSelect: (id?: string) => void;
  onStop: (agent: Agent) => void; canStop: boolean; onClose: () => void;
}) {
  useLocale();
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const ordered = useMemo(() => {
    const children = new Map<string | null, Agent[]>();
    const ids = new Set(agents.map(agent => agent.id));
    for (const agent of agents) {
      const parent = agent.parent_id && ids.has(agent.parent_id) ? agent.parent_id : null;
      children.set(parent, [...(children.get(parent) ?? []), agent]);
    }
    const visited = new Set<string>();
    const rows: { agent: Agent; depth: number; hasChildren: boolean }[] = [];
    const walk = (agent: Agent, depth: number) => {
      if (visited.has(agent.id)) return;
      visited.add(agent.id);
      rows.push({ agent, depth, hasChildren: Boolean(children.get(agent.id)?.length) });
      if (!collapsed.has(agent.id)) children.get(agent.id)?.forEach(child => walk(child, depth + 1));
    };
    children.get(null)?.forEach(agent => walk(agent, 0));
    // Malformed cyclic/orphan data remains inspectable without recursive loops.
    if (!rows.length) agents.forEach(agent => walk(agent, 0));
    return rows;
  }, [agents, collapsed]);
  const current = agents.find(agent => agent.id === selected);
  return <aside className="agents-panel"><div className="panel-heading"><GitBranch size={14} /><strong>{t('workspace-collaboration')}</strong><span>{agents.length}</span><button className="icon-button" aria-label={t('workspace-hide-collaboration-panel')} onClick={onClose}><PanelRightClose size={16} /></button></div>
    <button className={`agent-all ${!selected ? 'selected' : ''}`} onClick={() => onSelect()}><LayoutList size={15} />{t('workspace-all-progress')}</button>
    <div className="agent-list">{ordered.map(({ agent, depth, hasChildren }) => <div className="agent-row" key={agent.id} style={{ paddingLeft: Math.min(depth, 6) * 12 }}>
      <button className="agent-chevron" aria-label={collapsed.has(agent.id) ? t('workspace-expand', { v0: agent.name }) : t('workspace-collapse', { v0: agent.name })} disabled={!hasChildren}
        onClick={() => setCollapsed(value => { const next = new Set(value); if (next.has(agent.id)) next.delete(agent.id); else next.add(agent.id); return next; })}>
        {hasChildren ? collapsed.has(agent.id) ? <ChevronRight size={12} /> : <ChevronDown size={12} /> : <span />}
      </button>
      <button className={`agent-card ${selected === agent.id ? 'selected' : ''}`} onClick={() => onSelect(agent.id)}>
        <div className="agent-card-name"><Bot size={14} /><strong>{agent.name}</strong></div>
        <p title={agent.task}>{agent.task}</p><Status value={agent.status} />
      </button>
    </div>)}</div>
    {current && <div className="agent-inspector"><div className="detail-caption">{labels[current.role] ?? current.role} {t('workspace-generation-label', { count: current.generation })}</div>
      <h4>{current.name}</h4><p>{current.task}</p>{current.scope && <p className="muted">{t('workspace-scope')}{current.scope}</p>}
      <div className="metric-row"><span>{t('workspace-turn-count', { count: current.turns })}</span><span>{compact(current.total_tokens)} tokens</span></div>
      {current.error && <p className="error">{current.error}</p>}
      {current.summary && <p className="agent-summary">{current.summary}</p>}
      {!terminal(current.status) && <button className="danger subtle" disabled={!canStop} onClick={() => onStop(current)}>{t('workspace-stop-this-agent')}</button>}
    </div>}
  </aside>;
}
