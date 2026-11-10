import { useState } from 'react';
import brandIcon from '../../../icons/icon.svg';
import { ChevronRight, Container, Download, Plus, Search, Settings2 } from 'lucide-react';
import type { Host, Run } from '../types';
import { fileName, Status } from './Status';

export function Sidebar({ runs, selected, host, onSelect, onNew, onSettings, onUpdates, onWorkbench, busy }: {
  runs: Run[]; selected?: string | null; host?: Host; onSelect: (id: string) => void;
  onNew: () => void; onSettings: () => void; onUpdates: () => void; onWorkbench: () => void; busy: boolean;
}) {
  const [search, setSearch] = useState('');
  const visible = runs.filter(run => `${run.target_path} ${run.task}`.toLowerCase().includes(search.toLowerCase()));
  return <aside className="sidebar"><div className="brand"><span className="brand-mark"><img src={brandIcon} alt="" width={32} height={32} /></span><div><strong>Wukong</strong><span>分析工作区</span></div><span className="local-indicator" title="本地工作区" /></div>
    <button className="new-task" onClick={onNew} disabled={!host}><Plus size={16} />新建分析<span>{navigator.userAgent.includes('Mac') ? '⌘ N' : 'Ctrl N'}</span></button>
    <div className="sidebar-label">任务记录 <span>{runs.length}</span></div>
    <div className="search"><Search size={13} /><input aria-label="搜索任务" placeholder="搜索任务…" value={search} onChange={event => setSearch(event.target.value)} /></div>
    <nav className="run-list" aria-label="分析任务">{visible.map(run => <button className={`run-card ${run.id === selected ? 'selected' : ''}`} key={run.id} onClick={() => onSelect(run.id)}>
      <div className="run-title"><strong title={run.target_path}>{fileName(run.target_path)}</strong>{run.id === selected && <ChevronRight size={13} />}</div>
      <p>{run.task}</p><div className="run-meta"><Status value={run.status} /><time>{new Date(run.created_at).toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' })}</time></div>
    </button>)}</nav>
    {!visible.length && <p className="sidebar-empty">{runs.length ? '没有匹配的任务' : '创建任务，开始分析本地文件'}</p>}
    <div className="sidebar-footer"><button onClick={onUpdates} disabled={!host}><Download size={16} />更新与环境</button><button onClick={onWorkbench} disabled={!host || busy}><Container size={16} />Pwn 环境</button><button onClick={onSettings} disabled={!host}><Settings2 size={16} />工作区设置</button><div className="workspace-path" title={host?.state_dir}>{host?.state_dir ?? '正在连接本地引擎…'}</div></div>
  </aside>;
}
