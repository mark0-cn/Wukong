import { t, useLocale, currentLocale } from '../i18n';
import { useState } from 'react';
import brandIcon from '../../../icons/icon.svg';
import { ChevronRight, Container, Download, Plus, Search, Settings2 } from 'lucide-react';
import type { Host, Run } from '../types';
import { fileName, Status } from './Status';

export function Sidebar({ names, runs, selected, host, onSelect, onNew, onSettings, onUpdates, onWorkbench, busy }: {
  names: Record<string, string>; runs: Run[]; selected?: string | null; host?: Host; onSelect: (id: string) => void;
  onNew: () => void; onSettings: () => void; onUpdates: () => void; onWorkbench: () => void; busy: boolean;
}) {
  useLocale();
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState('all');
  const active = (run: Run) => ['queued', 'running', 'waiting'].includes(run.status);
  const visible = runs.filter(run => (filter === 'all' || (filter === 'active' ? active(run) : !active(run))) && `${names[run.id] ?? ''} ${run.target_path} ${run.task}`.toLowerCase().includes(search.toLowerCase())).sort((a, b) => b.created_at.localeCompare(a.created_at));
  const date = (run: Run) => new Date(run.created_at).toLocaleDateString(currentLocale(), { year: 'numeric', month: 'short', day: 'numeric' });
  return <aside className="sidebar"><div className="brand"><span className="brand-mark"><img src={brandIcon} alt="" width={32} height={32} /></span><div><strong>Wukong</strong><span>{t('workspace-analysis-workspace')}</span></div><span className="local-indicator" title={t('workspace-local-workspace')} /></div>
    <button className="new-task" onClick={onNew} disabled={!host}><Plus size={16} />{t('workspace-new-analysis')}<span>{navigator.userAgent.includes('Mac') ? '⌘ N' : 'Ctrl N'}</span></button>
    <div className="sidebar-label">{t('workspace-task-history')}<span>{runs.length}</span></div>
    <div className="search"><Search size={13} /><input aria-label={t('workspace-search-tasks')} placeholder={t('workspace-search-tasks-2')} value={search} onChange={event => setSearch(event.target.value)} /></div>
    <select className="task-filter" aria-label={t('ui-filter-tasks')} value={filter} onChange={event => setFilter(event.target.value)}><option value="all">{t('ui-all-tasks')}</option><option value="active">{t('ui-active-tasks')}</option><option value="history">{t('ui-history-tasks')}</option></select>
    <nav className="run-list" aria-label={t('workspace-analysis-task')}>{visible.map((run, index) => <div key={run.id}>{(!index || date(visible[index - 1]) !== date(run)) && <div className="run-date">{date(run)}</div>}<button className={`run-card ${run.id === selected ? 'selected' : ''}`} key={run.id} onClick={() => onSelect(run.id)}>
      <div className="run-title"><strong title={run.target_path}>{names[run.id] || fileName(run.target_path)}</strong>{run.id === selected && <ChevronRight size={13} />}</div>
      {names[run.id] && <span className="task-filename">{fileName(run.target_path)}</span>}<p>{run.task}</p><div className="run-meta"><Status value={run.status} /><time>{new Date(run.created_at).toLocaleTimeString(currentLocale(), { hour: '2-digit', minute: '2-digit' })}</time></div>
    </button></div>)}</nav>
    {!visible.length && <p className="sidebar-empty">{runs.length ? t('workspace-no-matching-tasks') : t('workspace-create-a-task-to-analyze-a-local')}</p>}
    <div className="sidebar-footer"><button onClick={onUpdates} disabled={!host}><Download size={16} />{t('workspace-updates-and-environment')}</button><button onClick={onWorkbench} disabled={!host || busy}><Container size={16} />{t('workspace-pwn-environment')}</button><button onClick={onSettings} disabled={!host}><Settings2 size={16} />{t('settings-workspace-settings')}</button><div className="workspace-path" title={host?.state_dir}>{host?.state_dir ?? t('workspace-connecting-to-local-engine')}</div></div>
  </aside>;
}
