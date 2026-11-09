import { useCallback, useEffect, useRef, useState } from 'react';
import { Activity, ArrowRight, Bot, CircleAlert, Download, FileSearch, GitBranch, MessageSquare, Pause, Play, RefreshCw, Sparkles, X } from 'lucide-react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { api, errorText, subscribe } from './bridge';
import { useWorkspace } from './workspace';
import type { Agent, Budget, Run } from './types';
import { Agents } from './components/Agents';
import { Composer, Conversation, type Draft } from './components/Conversation';
import { Findings } from './components/Findings';
import { Markdown } from './components/Markdown';
import { Modal } from './components/Modal';
import { Workbench } from './components/Workbench';
import { Settings } from './components/Settings';
import { Sidebar } from './components/Sidebar';
import { compact, fileName, StateIcon, Status, terminal } from './components/Status';
import { defaultTask, NewTask, ResumeTask } from './components/TaskDialogs';

type Confirmation = { kind: 'close'; busy: boolean } | { kind: 'stop'; agent: Agent; run: string; epoch: number };
export default function App() {
  const workspace = useWorkspace();
  const { data, host } = workspace;
  const [tab, setTab] = useState<'conversation' | 'findings' | 'overview'>('conversation');
  const [showAgents, setShowAgents] = useState(true);
  const [newTask, setNewTask] = useState(false);
  const [settings, setSettings] = useState(false);
  const [workbench, setWorkbench] = useState(false);
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [target, setTarget] = useState('');
  const [task, setTask] = useState(defaultTask);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [resume, setResume] = useState<Run>();
  const [confirmation, setConfirmation] = useState<Confirmation>();
  const [pending, setPending] = useState(false);
  const [closing, setClosing] = useState(false);
  const [error, setError] = useState('');
  const [dismissed, setDismissed] = useState<number>();
  const selectedRun = data?.runs.find(run => run.id === data.selected);
  const selectedAgent = data?.agents.find(agent => agent.id === workspace.agent);
  const recipient = selectedAgent ?? data?.agents.find(agent => agent.id === selectedRun?.root_id);
  const draftKey = `${host?.state_dir}|${data?.selected}|${recipient?.id}`;
  const draft = drafts[draftKey] ?? { text: '', followUp: false };
  const busy = Boolean(host?.execution || host?.pending_operations || pending);
  const anyDraft = settingsDirty || Object.values(drafts).some(value => value.text.trim()) || Boolean(target.trim() || task !== defaultTask);
  const closeState = useRef({ busy, anyDraft }); closeState.current = { busy, anyDraft };
  const close = async () => {
    setClosing(true); setError('');
    try { await api('shutdown_workspace'); }
    catch (error) { setError(errorText(error)); setClosing(false); }
  };
  useEffect(() => {
    let alive = true; const disposers: (() => void)[] = [];
    const register = (promise: Promise<() => void>) => { void promise.then(dispose => { if (alive) disposers.push(dispose); else dispose(); }).catch(() => {}); };
    register(subscribe<{ busy: boolean }>('close-requested', event => {
      if (closeState.current.busy || closeState.current.anyDraft || event.payload.busy) setConfirmation({ kind: 'close', busy: closeState.current.busy || event.payload.busy });
      else void close();
    }));
    try {
      register(getCurrentWebview().onDragDropEvent(event => {
        if (event.payload.type === 'drop' && event.payload.paths[0]) { setTarget(event.payload.paths[0]); setNewTask(true); }
      }));
    } catch { /* A browser preview has no native drop surface. */ }
    const keyboard = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'n') { event.preventDefault(); setNewTask(true); }
    };
    window.addEventListener('keydown', keyboard);
    return () => { alive = false; disposers.forEach(dispose => dispose()); window.removeEventListener('keydown', keyboard); };
  }, []);
  const command = async (action: () => Promise<unknown>) => {
    setPending(true); setError('');
    try { await action(); workspace.refresh(); }
    catch (error) { setError(errorText(error)); }
    finally { setPending(false); }
  };
  const selectAgent = (id?: string) => { workspace.selectAgent(id); setTab('conversation'); };
  const onDirty = useCallback((value: boolean) => setSettingsDirty(value), []);
  const start = async () => {
    if (!host) return;
    const id = await api<string>('start_run', { epoch: host.epoch, target, task });
    workspace.selectRun(id); workspace.refresh(); setNewTask(false); setTarget(''); setTask(defaultTask); setTab('conversation');
  };
  const resumeRun = async (budget: Budget) => {
    if (!host || !resume) return;
    const id = await api<string>('resume_run', { epoch: host.epoch, run: resume.id, budget });
    workspace.selectRun(id); workspace.refresh(); setResume(undefined); setTab('conversation');
  };
  return <div className="app-shell">
    {workbench && host && <Workbench epoch={host.epoch} onClose={() => setWorkbench(false)} onChanged={workspace.refresh} />}
    <Sidebar runs={data?.runs ?? []} selected={data?.selected} host={host} onSelect={id => { workspace.selectRun(id); setTab('conversation'); }}
      onNew={() => setNewTask(true)} onSettings={() => setSettings(true)} />
    <main className="workspace-main">
      <header className="workspace-header"><div className="workspace-title"><span className="breadcrumb">本地工作区 <span>/</span> 分析任务</span><div><h1>{selectedRun ? fileName(selectedRun.target_path) : '开始一项新的分析'}</h1>{selectedRun && <Status value={selectedRun.status} />}</div></div>
        <div className="header-actions"><button disabled={busy || closing} onClick={() => setWorkbench(true)}>Pwn 环境</button><button className="icon-button" aria-label="刷新任务" onClick={workspace.refresh}><RefreshCw size={15} className={workspace.loading ? 'spin' : ''} /></button>
          {selectedRun && host && <><button disabled={pending || closing} onClick={() => { void command(() => api('export_report', { epoch: host.epoch, run: selectedRun.id })); }}><Download size={14} /><span>导出</span></button>
            {['running', 'waiting', 'queued'].includes(selectedRun.status) ? <button disabled={pending || closing} onClick={() => { void command(() => api('pause_run', { epoch: host.epoch, run: selectedRun.id })); }}><Pause size={14} />暂停</button> :
              <button className="primary" disabled={busy || closing} onClick={() => setResume(selectedRun)}><Play size={14} />恢复分析</button>}</>}
        </div></header>
      {(error || workspace.error) && <div className="error-banner" role="alert"><CircleAlert size={16} /><span>{error || workspace.error}</span><button className="icon-button" aria-label="重试连接" onClick={() => { setError(''); workspace.refresh(); }}><RefreshCw size={14} /></button></div>}
      {host?.notice && dismissed !== host.notice.id && <div className={`notice ${host.notice.error ? 'notice-error' : ''}`} role="status"><span>{host.notice.message}</span><button className="icon-button" aria-label="关闭通知" onClick={() => setDismissed(host.notice!.id)}><X size={13} /></button></div>}
      {selectedRun && <><nav className="workspace-tabs" aria-label="工作区视图">
        <button className={tab === 'conversation' ? 'active' : ''} onClick={() => setTab('conversation')}><MessageSquare size={15} />工作进展</button>
        <button className={tab === 'findings' ? 'active' : ''} onClick={() => setTab('findings')}><FileSearch size={15} />发现与证据<span>{data?.findings.length ?? 0}</span></button>
        <button className={tab === 'overview' ? 'active' : ''} onClick={() => setTab('overview')}><Activity size={15} />任务概览</button>
        <div className="tab-spacer" /><button className={`agents-toggle ${showAgents ? 'enabled' : ''}`} aria-label="切换协作任务面板" onClick={() => setShowAgents(value => !value)}><GitBranch size={15} />协作任务</button>
      </nav>
        {data && <div className={`work-status ${['failed', 'crashed'].includes(selectedRun.status) ? 'work-error' : ''}`}>
          <span className="work-status-icon"><StateIcon state={data.active_tools.some(tool => tool.state === 'running') ? 'running' : selectedRun.status} /></span>
          <div><strong>{data.active_tools.length ? data.active_tools[0].title : selectedRun.status === 'running' ? '分析正在进行' : selectedRun.status === 'waiting' ? '等待协作任务汇报' : selectedRun.status === 'budget_paused' ? '进度已保存，可以恢复分析' : terminal(selectedRun.status) ? '本轮工作已结束' : '等待开始工作'}</strong>
            <p>{data.active_tools.length ? `${data.active_tools[0].target || data.active_tools[0].purpose}${data.active_tools.length > 1 ? ` · 另有 ${data.active_tools.length - 1} 项操作` : ''}` : selectedRun.task}</p></div>
          <div className="budget-compact"><span>{compact(selectedRun.total_tokens)} / {compact(selectedRun.budget.max_total_tokens)} tokens</span><span>{data.agents.length} 个 Agent</span></div>
        </div>}
      </>}
      <div className="workspace-content">{data && host && selectedRun ? <>
        <div className="primary-panel">{tab === 'conversation' ? <Conversation key={`${host.epoch}:${data.selected}:${workspace.agent ?? 'all'}`} data={data} host={host} agent={selectedAgent} onAgent={selectAgent} loadEarlier={workspace.loadEarlier} limit={workspace.historyLimit}
          composer={<Composer key={draftKey} target={recipient} draft={draft} busy={pending || closing} onDraft={value => setDrafts(previous => ({ ...previous, [draftKey]: value }))}
            send={async (draft, agent) => {
              await api('send_message', { epoch: host.epoch, run: selectedRun.id, agent: agent.id, generation: agent.generation, text: draft.text, followUp: draft.followUp });
              setDrafts(previous => previous[draftKey]?.text === draft.text && previous[draftKey]?.followUp === draft.followUp ? { ...previous, [draftKey]: { ...draft, text: '' } } : previous);
              workspace.refresh();
            }} />} /> : tab === 'findings' ? <Findings key={`${host.epoch}:${data.selected}`} findings={data.findings} epoch={host.epoch} run={selectedRun.id} onAgent={selectAgent} /> :
              <div className="overview-scroll"><section className="overview-summary"><span className="eyebrow">ANALYSIS WORKSPACE</span><h2>{fileName(selectedRun.target_path)}</h2><p className="muted path">{selectedRun.target_path}</p><Markdown text={selectedRun.task} />
                <div className="metrics"><div><span>Tokens</span><strong>{compact(selectedRun.total_tokens)}</strong></div><div><span>累计费用</span><strong>${selectedRun.cost_usd.toFixed(4)}</strong></div><div><span>协作 Agent</span><strong>{data.agents.length}</strong></div><div><span>发现</span><strong>{data.findings.length}</strong></div></div>
                {selectedRun.summary && <><h3>任务总结</h3><Markdown text={selectedRun.summary} /></>}
              </section><section className="overview-agents"><h3>协作任务</h3>{data.agents.map(agent => <button key={agent.id} onClick={() => selectAgent(agent.id)}><Bot size={18} /><div><strong>{agent.name}</strong><p>{agent.task}</p></div><Status value={agent.status} /><ArrowRight size={14} /></button>)}</section>
              {data.notes.length > 0 && <section className="shared-notes"><h3>共享分析笔记</h3>{data.notes.map(note => <details key={note.id}><summary>{note.title}</summary><Markdown text={note.body} /></details>)}</section>}</div>}
        </div>
        {showAgents && <Agents agents={data.agents} selected={workspace.agent} onSelect={selectAgent} canStop={!pending && !closing} onStop={agent => setConfirmation({ kind: 'stop', agent, run: selectedRun.id, epoch: host.epoch })} />}
      </> : <div className="welcome"><div className="welcome-icon"><Sparkles size={28} /></div><span className="eyebrow">RE AGENT</span><h2>{workspace.loading ? '正在载入工作区…' : '让分析过程清晰可见'}</h2><p>从本地文件开始，让 Agent 拆解任务、执行分析，<br />并带回可以追溯的结论与证据。</p><button className="primary" disabled={!host} onClick={() => setNewTask(true)}>新建分析<ArrowRight size={15} /></button><div className="welcome-features"><span><MessageSquare size={15} />实时工作进展</span><span><GitBranch size={15} />多 Agent 协作</span><span><FileSearch size={15} />证据追溯</span></div></div>}</div>
      <footer className="workspace-footer"><span><span className="connection-dot" />{workspace.error ? '连接异常' : '本地引擎'}{host && <> · {host.model}</>}</span><span>{host?.execution ? host.execution.phase === 'starting' ? '正在准备分析…' : '后台分析运行中' : '所有进展保存在本地'}</span></footer>
    </main>
    {host && <Settings open={settings} host={host} onClose={() => setSettings(false)} onSaved={workspace.reset} onDirty={onDirty} />}
    {newTask && <NewTask target={target} task={task} setTarget={setTarget} setTask={setTask} busy={busy} onClose={() => setNewTask(false)} onStart={start} />}
    {resume && <ResumeTask key={resume.id} run={resume} onClose={() => setResume(undefined)} onResume={resumeRun} />}
    {confirmation && <Modal title={confirmation.kind === 'close' ? '关闭工作区' : `停止 ${confirmation.agent.name}？`} description={confirmation.kind === 'close' ? `${confirmation.busy ? '正在运行的分析会先暂停并保存，文件操作结束后再退出。' : ''}${anyDraft ? '未发送的消息和未保存的设置草稿会丢失。' : ''}` : '此 Agent 及其子任务将停止；已有分析结果和证据会保留。'} onClose={() => { if (!closing) setConfirmation(undefined); }}>
      {closing && <p className="muted"><StateIcon state="running" />正在等待分析和文件操作安全结束…</p>}
      <div className="modal-footer"><button disabled={closing || pending} onClick={() => setConfirmation(undefined)}>取消</button><button className="danger" disabled={closing || pending} onClick={() => {
        if (confirmation.kind === 'close') void close();
        else { const captured = confirmation; void command(async () => { await api('stop_agent', { epoch: captured.epoch, run: captured.run, agent: captured.agent.id }); setConfirmation(undefined); }); }
      }}>{confirmation.kind === 'close' ? closing ? '正在保存…' : '保存进度并退出' : '停止此 Agent'}</button></div>
    </Modal>}
  </div>;
}
