import { t, useLocale, currentLocale } from '../i18n';
import { useState } from 'react';
import brandIcon from '../../../icons/icon.svg';
import { ChevronRight, Container, Download, Plus, Search, Settings2 } from 'lucide-react';
import type { Host, Run } from '../types';
import { fileName, Status } from './Status';

export function Sidebar({ runs, selected, host, onSelect, onNew, onSettings, onUpdates, onWorkbench, busy }: {
  runs: Run[]; selected?: string | null; host?: Host; onSelect: (id: string) => void;
  onNew: () => void; onSettings: () => void; onUpdates: () => void; onWorkbench: () => void; busy: boolean;
}) {
  useLocale();
  const [search, setSearch] = useState('');
  const visible = runs.filter(run => `${run.target_path} ${run.task}`.toLowerCase().includes(search.toLowerCase()));
  return <aside className="sidebar"><div className="brand"><span className="brand-mark"><img src={brandIcon} alt="" width={32} height={32} /></span><div><strong>Wukong</strong><span>{t('workspace-analysis-workspace')}</span></div><span className="local-indicator" title={t('workspace-local-workspace')} /></div>
    <button className="new-task" onClick={onNew} disabled={!host}><Plus size={16} />{t('workspace-new-analysis')}<span>{navigator.userAgent.includes('Mac') ? '⌘ N' : 'Ctrl N'}</span></button>
    <div className="sidebar-label">{t('workspace-task-history')}<span>{runs.length}</span></div>
    <div className="search"><Search size={13} /><input aria-label={t('workspace-search-tasks')} placeholder={t('workspace-search-tasks-2')} value={search} onChange={event => setSearch(event.target.value)} /></div>
    <nav className="run-list" aria-label={t('workspace-analysis-task')}>{visible.map(run => <button className={`run-card ${run.id === selected ? 'selected' : ''}`} key={run.id} onClick={() => onSelect(run.id)}>
      <div className="run-title"><strong title={run.target_path}>{fileName(run.target_path)}</strong>{run.id === selected && <ChevronRight size={13} />}</div>
      <p>{run.task}</p><div className="run-meta"><Status value={run.status} /><time>{new Date(run.created_at).toLocaleDateString(currentLocale(), { month: '2-digit', day: '2-digit' })}</time></div>
    </button>)}</nav>
    {!visible.length && <p className="sidebar-empty">{runs.length ? t('workspace-no-matching-tasks') : t('workspace-create-a-task-to-analyze-a-local')}</p>}
    <div className="sidebar-footer"><button onClick={onUpdates} disabled={!host}><Download size={16} />{t('workspace-updates-and-environment')}</button><button onClick={onWorkbench} disabled={!host || busy}><Container size={16} />{t('workspace-pwn-environment')}</button><button onClick={onSettings} disabled={!host}><Settings2 size={16} />{t('settings-workspace-settings')}</button><div className="workspace-path" title={host?.state_dir}>{host?.state_dir ?? t('workspace-connecting-to-local-engine')}</div></div>
  </aside>;
}
