import { t, useLocale } from '../i18n';
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { Bot, ChevronDown, ChevronRight, GitBranch, LayoutList, PanelRightClose, ScanSearch, ShieldCheck, CornerUpLeft } from 'lucide-react';
import type { Agent } from '../types';
import { compact, labels, Status, terminal } from './Status';
import { useViewPreference } from '../hooks/useViewPreference';
import { agentPath, buildAgentTree, visibleAgentRows, type Descendants, type TreeRow } from './agentTree';

interface TreeView { collapsed: string[]; focus: string | null }
const validView = (value: unknown): value is TreeView => {
  if (!value || typeof value !== 'object') return false;
  const view = value as Partial<TreeView>;
  return Array.isArray(view.collapsed) && view.collapsed.every(id => typeof id === 'string') && (view.focus === null || typeof view.focus === 'string');
};
function RoleIcon({ role }: { role: string }) {
  return role === 'root' ? <GitBranch size={14} /> : role === 'verifier' ? <ShieldCheck size={14} /> : role === 'analyst' ? <ScanSearch size={14} /> : <Bot size={14} />;
}
function roleLabel(role: string) {
  return role === 'root' ? t('tree-role-root') : role === 'analyst' ? t('tree-role-analyst') : role === 'verifier' ? t('tree-role-verifier') : role;
}
function countLabel(count: Descendants) {
  return [t('tree-child-count', { count: count.total }), count.running ? t('tree-running-count', { count: count.running }) : '', count.failed ? t('tree-failed-count', { count: count.failed }) : '', count.waiting ? t('tree-waiting-count', { count: count.waiting }) : '', count.completed ? t('tree-completed-count', { count: count.completed }) : ''].filter(Boolean).join(' · ');
}

export function Agents({ scopeKey, agents, selected, onSelect, onStop, canStop, onClose }: {
  scopeKey: string; agents: Agent[]; selected?: string; onSelect: (id?: string) => void;
  onStop: (agent: Agent) => void; canStop: boolean; onClose: () => void;
}) {
  useLocale();
  const [view, setView] = useViewPreference<TreeView>(`agent-tree:${scopeKey}`, { collapsed: [], focus: null }, validView);
  const collapsed = new Set(view.collapsed);
  const tree = useMemo(() => buildAgentTree(agents), [agents]);
  const focus = view.focus && tree.nodes.has(view.focus) ? view.focus : undefined;
  const rows = visibleAgentRows(tree, collapsed, focus);
  const path = agentPath(tree, selected);
  const selectedPath = new Set(path);
  const [active, setActive] = useState<string>();
  const elements = useRef(new Map<string, HTMLDivElement>());
  const previousSelection = useRef(selected);
  useEffect(() => {
    if (previousSelection.current === selected) return;
    previousSelection.current = selected;
    if (!selected || !tree.nodes.has(selected)) return;
    const ancestors = agentPath(tree, selected);
    setActive(selected);
    if (ancestors.some(id => id !== selected && view.collapsed.includes(id)) || (focus && !ancestors.includes(focus))) {
      setView(previous => ({ collapsed: previous.collapsed.filter(id => id === selected || !ancestors.includes(id)), focus: focus && !ancestors.includes(focus) ? null : previous.focus }));
    }
  }, [selected, tree, focus, view.collapsed, setView]);
  const current = selected ? tree.nodes.get(selected) : undefined;
  const tabStop = rows.some(row => row.id === active) ? active : [...path].reverse().find(id => rows.some(row => row.id === id)) ?? rows[0]?.id;
  const toggle = (id: string) => setView(previous => ({ ...previous, collapsed: previous.collapsed.includes(id) ? previous.collapsed.filter(value => value !== id) : [...previous.collapsed, id] }));
  const focusBranch = (id: string | null) => {
    setView(previous => ({ collapsed: previous.collapsed.filter(value => value !== id), focus: id }));
    if (id) { setActive(id); requestAnimationFrame(() => elements.current.get(id)?.focus()); }
  };
  const keyboard = (event: KeyboardEvent<HTMLDivElement>, row: TreeRow) => {
    const index = rows.findIndex(item => item.id === row.id);
    let next: string | undefined;
    if (event.key === 'ArrowDown') next = rows[Math.min(index + 1, rows.length - 1)]?.id;
    else if (event.key === 'ArrowUp') next = rows[Math.max(index - 1, 0)]?.id;
    else if (event.key === 'Home') next = rows[0]?.id;
    else if (event.key === 'End') next = rows.at(-1)?.id;
    else if (event.key === 'ArrowRight') {
      if (row.limited) focusBranch(row.id);
      else if (collapsed.has(row.id)) toggle(row.id);
      else next = tree.children.get(row.id)?.[0];
    } else if (event.key === 'ArrowLeft') {
      if (tree.children.has(row.id) && !collapsed.has(row.id) && !row.limited) toggle(row.id);
      else next = row.depth ? tree.parents.get(row.id) ?? undefined : undefined;
    } else if (event.key === 'Enter' || event.key === ' ') onSelect(row.id);
    else return;
    event.preventDefault(); event.stopPropagation();
    if (next) { setActive(next); elements.current.get(next)?.focus(); }
  };
  return <aside className="agents-panel"><div className="panel-heading"><GitBranch size={14} /><strong>{t('workspace-collaboration')}</strong><span>{agents.length}</span><button className="icon-button" aria-label={t('workspace-hide-collaboration-panel')} onClick={onClose}><PanelRightClose size={16} /></button></div>
    <button className={`agent-all ${!selected ? 'selected' : ''}`} onClick={() => onSelect()}><LayoutList size={15} />{t('workspace-all-progress')}</button>
    {focus && <div className="tree-focus"><button className="subtle" onClick={() => focusBranch(tree.parents.get(focus) ?? null)}><CornerUpLeft size={13} />{t('tree-up-level')}</button><button className="subtle" onClick={() => focusBranch(null)}>{t('tree-show-all')}</button><p title={agentPath(tree, focus).map(id => tree.nodes.get(id)!.name).join(' › ')}>{t('tree-focused-branch')}: {tree.nodes.get(focus)!.name}</p></div>}
    <div className="agent-list" role="tree" aria-label={t('tree-label')}>
      {rows.map(row => {
        const agent = tree.nodes.get(row.id)!;
        const children = tree.children.get(row.id) ?? [];
        const closed = collapsed.has(row.id) || row.limited;
        const count = tree.counts.get(row.id)!;
        return <div className={`agent-row ${row.depth === 0 ? 'agent-tree-root' : ''} ${selectedPath.has(row.id) ? 'agent-path' : ''}`} key={row.id}
          ref={element => { if (element) elements.current.set(row.id, element); else elements.current.delete(row.id); }}
          role="treeitem" aria-label={`${agent.name}, ${roleLabel(agent.role)}, ${labels[agent.status] ?? agent.status}`}
          aria-level={row.depth + 1} aria-posinset={row.position} aria-setsize={row.size} aria-selected={selected === row.id}
          aria-expanded={children.length ? !closed : undefined} tabIndex={tabStop === row.id ? 0 : -1}
          onFocus={() => setActive(row.id)} onKeyDown={event => keyboard(event, row)} style={{ paddingLeft: row.depth * 18 }}>
          <span className="tree-rails" aria-hidden="true">{Array.from({ length: row.depth }, (_, level) => <span key={level} style={{ left: level * 18 + 8 }} className={`tree-rail ${level === row.depth - 1 ? `tree-branch ${row.last ? 'tree-last' : ''}` : row.rails[level + 1] ? 'tree-continue' : ''}`} />)}</span>
          <div className="agent-node">
            {children.length ? <button className="agent-chevron" tabIndex={-1} aria-label={row.limited ? t('tree-focus-branch', { name: agent.name }) : closed ? t('workspace-expand', { v0: agent.name }) : t('workspace-collapse', { v0: agent.name })}
              onClick={() => { if (row.limited) focusBranch(row.id); else toggle(row.id); }}>{closed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}</button> : <span className="agent-chevron-placeholder" />}
            <button tabIndex={-1} className={`agent-card ${selected === row.id ? 'selected' : ''}`} onClick={() => onSelect(row.id)} title={agent.name}>
              <span className="agent-card-name"><RoleIcon role={agent.role} /><strong>{agent.name}</strong><span className="agent-role">{roleLabel(agent.role)}</span></span>
              <span className="agent-card-meta"><Status value={agent.status} />{agent.scope && <span className="agent-scope" title={agent.scope}>· {agent.scope}</span>}</span>
              {agent.error && <span className="agent-inline-error" title={agent.error}>{agent.error}</span>}
              {closed && count.total > 0 && <span className={`agent-descendants ${count.failed ? 'has-failures' : ''}`}>{countLabel(count)}</span>}
              {closed && selected !== row.id && selectedPath.has(row.id) && <span className="agent-hidden-selection">{t('tree-contains-selection')}</span>}
              {tree.issues.has(row.id) && <span className="agent-inline-error">{t('tree-incomplete-parent')}</span>}
            </button>
          </div>
          {row.limited && <button className="tree-drill subtle" onClick={() => focusBranch(row.id)}>{t('tree-focus-branch', { name: agent.name })}<ChevronRight size={12} /></button>}
        </div>;
      })}
    </div>
    {current && <div className="agent-inspector"><div className="detail-caption">{t('tree-selection-details')}</div>
      <nav className="agent-breadcrumb" aria-label={t('tree-parent-path')}>{path.map((id, index) => <span key={id}>{index > 0 && <span aria-hidden="true"> › </span>}<button className="subtle" onClick={() => onSelect(id)} title={tree.nodes.get(id)!.name}>{tree.nodes.get(id)!.name}</button></span>)}</nav>
      <h4>{current.name}</h4><div className="agent-inspector-state"><span className="agent-role">{roleLabel(current.role)}</span><Status value={current.status} /><span>{t('workspace-generation-label', { count: current.generation })}</span></div>
      <p>{current.task}</p>{current.scope && <p className="muted">{t('workspace-scope')}{current.scope}</p>}
      <div className="metric-row"><span>{t('workspace-turn-count', { count: current.turns })}</span><span>{compact(current.total_tokens)} tokens</span></div>
      {current.error && <p className="error">{current.error}</p>}
      {current.summary && <p className="agent-summary">{current.summary}</p>}
      {!terminal(current.status) && <button className="danger subtle" disabled={!canStop} onClick={() => onStop(current)}>{t('workspace-stop-this-agent')}</button>}
    </div>}
  </aside>;
}
