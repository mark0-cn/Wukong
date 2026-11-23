import { t, message, useLocale } from '../shared/i18n';
import brandIcon from '../../../icons/icon.svg';
import { useCallback, useRef, useState, type ReactNode } from 'react';
import { Activity, ArrowRight, Bot, CircleAlert, Download, FileSearch, Focus, Gauge, GitBranch, MessageSquare, PanelLeft, Pause, Play, RefreshCw, X } from 'lucide-react';
import { api, errorText, errorValue } from '../shared/api/bridge';
import { useWorkspace } from '../features/workspace/model';
import type { Agent, Budget, Run, Workspace } from '../shared/api/types';
import { Agents } from '../features/agents/Agents';
import { Composer, Conversation, type Draft } from '../features/conversation/Conversation';
import { Findings } from '../features/evidence/Findings';
import { Markdown } from '../shared/ui/Markdown';
import { Modal } from '../shared/ui/Modal';
import { Workbench } from '../features/maintenance/Workbench';
import { Updates } from '../features/maintenance/Updates';
import { WorkspaceStatus, WorkspaceFooter } from '../features/workspace/WorkspaceStatus';
import { TaskHeading } from '../features/workspace/TaskHeading';
import { useViewPreference, booleanPreference, optionalBooleanPreference, runNamesPreference } from '../shared/hooks/useViewPreference';
import { Settings } from '../features/settings/Settings';
import { Sidebar } from '../features/workspace/Sidebar';
import { WorkspaceSplit, useWideWorkspace } from '../features/workspace/WorkspaceSplit';
import { compact, fileName, StateIcon, Status } from '../shared/ui/Status';
import { defaultTask, NewTask, ResumeTask } from '../features/workspace/TaskDialogs';

import { useDesktopLifecycle } from './useDesktopLifecycle';
import { useUpdateController } from '../features/maintenance/useUpdateController';
import { useStartupUpdates } from '../features/maintenance/useStartupUpdates';
import { Observability } from '../features/observability/Observability';

export type WorkspaceTab = 'conversation' | 'findings' | 'overview' | 'observability';

type Confirmation = { kind: 'close'; busy: boolean } | { kind: 'stop'; agent: Agent; run: string; epoch: number };
export default function App() {
  useLocale();
  const workspace = useWorkspace();
  const { data, host } = workspace;
  const updateController = useUpdateController(host?.epoch, workspace.reset);
  const [tab, setTab] = useState<WorkspaceTab>('conversation');
  const wideWorkspace = useWideWorkspace();
  const [showAgents, setShowAgents] = useViewPreference<boolean | null>('agents-visible', null, optionalBooleanPreference);
  const agentsVisible = showAgents ?? wideWorkspace;
  const [showSidebar, setShowSidebar] = useViewPreference('sidebar-visible', true, booleanPreference);
  const [runNames, setRunNames] = useViewPreference<Record<string, string>>(`run-names:${host?.state_dir ?? ''}`, {}, runNamesPreference);
  const [focused, setFocused] = useState(false);
  const [newTask, setNewTask] = useState(false);
  const [settings, setSettings] = useState(false);
  const [workbench, setWorkbench] = useState<'tools' | 'challenges' | null>(null);
  const [updates, setUpdates] = useState(false);
  useStartupUpdates(host, useCallback(() => setUpdates(true), []));
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [target, setTarget] = useState('');
  const [task, setTask] = useState(defaultTask);
  const initialTask = useRef(task);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [resume, setResume] = useState<Run>();
  const [confirmation, setConfirmation] = useState<Confirmation>();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [dismissed, setDismissed] = useState<number>();
  const selectedRun = data?.runs.find(run => run.id === data.selected);
  const selectedAgent = data?.agents.find(agent => agent.id === workspace.agent);
  const recipient = selectedAgent ?? data?.agents.find(agent => agent.id === selectedRun?.root_id);
  const draftKey = `${host?.state_dir}|${data?.selected}|${recipient?.id}`;
  const draft = drafts[draftKey] ?? { text: '', followUp: false };
  const busy = Boolean(host?.execution || host?.pending_operations || updateController.working || pending);
  const anyDraft = settingsDirty || Object.values(drafts).some(value => value.text.trim()) || Boolean(target.trim() || task !== initialTask.current);
  const { close, closing } = useDesktopLifecycle({ busy, hasDraft: anyDraft,
    onConfirm: busy => setConfirmation({ kind: 'close', busy }),
    onTarget: path => { if (path) setTarget(path); setNewTask(true); }, onError: setError });
  const command = async (action: () => Promise<unknown>) => {
    setPending(true); setError('');
    try { await action(); workspace.refresh(); }
    catch (error) { setError(errorValue(error)); }
    finally { setPending(false); }
  };
  const selectAgent = (id?: string) => { workspace.selectAgent(id); setTab('conversation'); };
  const onDirty = useCallback((value: boolean) => setSettingsDirty(value), []);
  const start = async () => {
    if (!host) return;
    const id = await api('start_run', { epoch: host.epoch, target, task });
    workspace.selectRun(id); workspace.refresh(); setNewTask(false); setTarget(''); initialTask.current = defaultTask(); setTask(initialTask.current); setTab('conversation');
  };
  const resumeRun = async (budget: Budget) => {
    if (!host || !resume) return;
    const id = await api('resume_run', { epoch: host.epoch, run: resume.id, budget });
    workspace.selectRun(id); workspace.refresh(); setResume(undefined); setTab('conversation');
  };
  return <div className="app-shell">
    {updates && host && <Updates controller={updateController} epoch={host.epoch} onClose={() => setUpdates(false)} onChanged={workspace.reset} />}
    {workbench && host && <Workbench controller={updateController} mode={workbench} epoch={host.epoch} onClose={() => setWorkbench(null)} onChanged={workspace.refresh} />}
    <WorkspaceSplit id="task-navigation" label={t('workspace-resize-task-list')} leading visible={showSidebar && !focused}
      side={<Sidebar operation={updateController.status?.operation_id && (updateController.working || ['setup', 'prepare'].includes(updateController.status.action ?? '') || updateController.status.phase === 'error') ? { active: updateController.working, text: updateController.status.cancelling ? t('updates-cancelling') : updateController.status.message ? message(updateController.status.message) : t('updates-setup-working') } : undefined} names={runNames} runs={data?.runs ?? []} selected={data?.selected} host={host} onSelect={id => { workspace.selectRun(id); setTab('conversation'); }}
        onNew={() => setNewTask(true)} onSettings={() => setSettings(true)} onUpdates={() => { setWorkbench(null); setUpdates(true); }} onTools={() => { setUpdates(false); setWorkbench('tools'); }} onWorkbench={() => setWorkbench('challenges')} busy={busy || closing} />}>
    <main className="workspace-main">
      <header className="workspace-header"><button className="icon-button" title={t('workspace-toggle-task-list')} aria-label={t('workspace-toggle-task-list')} aria-pressed={showSidebar && !focused} onClick={() => { if (focused) { setFocused(false); setShowSidebar(true); } else setShowSidebar(value => !value); }}><PanelLeft size={18} /></button><div className="workspace-title">{selectedRun ? <TaskHeading key={selectedRun.id} run={selectedRun} name={runNames[selectedRun.id]} rename={value => setRunNames(previous => ({ ...previous, [selectedRun.id]: value }))} /> : <h1>{t('workspace-start-a-new-analysis')}</h1>}</div>
        <div className="header-actions"><button className="icon-button focus-toggle" title={focused ? t('workspace-exit-focus-mode') : t('workspace-focus-mode')} aria-label={t('workspace-focus-mode')} aria-pressed={focused} onClick={() => setFocused(value => !value)}><Focus size={17} /></button><button className="icon-button" aria-label={t('workspace-refresh-tasks')} onClick={workspace.refresh}><RefreshCw size={15} className={workspace.loading ? 'spin' : ''} /></button>
          {selectedRun && host && <><button disabled={pending || closing} onClick={() => { void command(() => api('export_report', { epoch: host.epoch, run: selectedRun.id })); }}><Download size={14} /><span>{t('workspace-export')}</span></button>
            {selectedRun.status === 'completed' ? <button className="primary" onClick={() => setTab('findings')}><FileSearch size={14} />{t('workspace-findings-and-evidence')}</button> : ['running', 'waiting', 'queued'].includes(selectedRun.status) ? <button disabled={pending || closing} onClick={() => { void command(() => api('pause_run', { epoch: host.epoch, run: selectedRun.id })); }}><Pause size={14} />{t('workspace-pause')}</button> :
              <button className="primary" disabled={busy || closing} onClick={() => setResume(selectedRun)}><Play size={14} />{t('workspace-resume-analysis')}</button>}</>}
        </div></header>
      {(error || workspace.error) && <div className="error-banner" role="alert"><CircleAlert size={16} /><span>{errorText(error || workspace.error)}</span><button className="icon-button" aria-label={t('workspace-retry-connection')} onClick={() => { setError(''); workspace.refresh(); }}><RefreshCw size={14} /></button></div>}
      {host?.notice && dismissed !== host.notice.id && <div className={`notice ${host.notice.error ? 'notice-error' : ''}`} role="status"><span>{message(host.notice.message)}</span><button className="icon-button" aria-label={t('workspace-dismiss-notification')} onClick={() => setDismissed(host.notice!.id)}><X size={13} /></button></div>}
      {selectedRun && <><WorkspaceTabs tab={tab} onTab={setTab} findingsCount={data?.findings.length ?? 0}
        agentsVisible={agentsVisible && !focused} showAgentToggle={tab !== 'observability'}
        onToggleAgents={() => { if (focused) { setFocused(false); setShowAgents(true); } else setShowAgents(value => !(value ?? wideWorkspace)); }} />
        {data && <WorkspaceStatus run={selectedRun} data={data} />}
      </>}
      <div className="workspace-content">{data && host && selectedRun ? <>
        <SelectedRunContent tab={tab} showAgents={tab !== 'observability' && agentsVisible && !focused}
          agents={<Agents key={`${host.state_dir}:${selectedRun.id}`} scopeKey={`${host.state_dir}:${selectedRun.id}`} agents={data.agents} selected={workspace.agent} onSelect={selectAgent} canStop={!pending && !closing} onClose={() => setShowAgents(false)} onStop={agent => setConfirmation({ kind: 'stop', agent, run: selectedRun.id, epoch: host.epoch })} />}
          conversation={<Conversation key={`${host.epoch}:${data.selected}:${workspace.agent ?? 'all'}`} data={data} host={host} agent={selectedAgent} onAgent={selectAgent} loadEarlier={workspace.loadEarlier} limit={workspace.historyLimit}
            composer={<Composer key={draftKey} target={recipient} draft={draft} busy={pending || closing} onDraft={value => setDrafts(previous => ({ ...previous, [draftKey]: value }))}
              send={async (draft, agent) => {
                await api('send_message', { epoch: host.epoch, run: selectedRun.id, agent: agent.id, generation: agent.generation, text: draft.text, followUp: draft.followUp });
                setDrafts(previous => previous[draftKey]?.text === draft.text && previous[draftKey]?.followUp === draft.followUp ? { ...previous, [draftKey]: { ...draft, text: '' } } : previous);
                workspace.refresh();
              }} />} />}
          findings={<Findings key={`${host.epoch}:${data.selected}`} findings={data.findings} epoch={host.epoch} run={selectedRun.id} onAgent={selectAgent} />}
          overview={<Overview data={data} run={selectedRun} onAgent={selectAgent} />}
          observability={<Observability key={`${host.epoch}:${selectedRun.id}`} epoch={host.epoch} run={selectedRun.id} runStatus={selectedRun.status} />} />
      </> : <div className="welcome"><div className="welcome-icon"><img src={brandIcon} alt="Wukong" width={64} height={64} /></div><span className="eyebrow">WUKONG</span><h2>{workspace.loading ? t('workspace-loading-workspace') : t('workspace-see-your-analysis-take-shape')}</h2><p>{t('workspace-welcome-description')}</p><button className="primary" disabled={!host} onClick={() => setNewTask(true)}>{t('workspace-new-analysis')}<ArrowRight size={15} /></button><div className="welcome-features"><span><MessageSquare size={15} />{t('workspace-live-progress')}</span><span><GitBranch size={15} />{t('workspace-agent-collaboration')}</span><span><FileSearch size={15} />{t('workspace-traceable-evidence')}</span></div></div>}</div>
      <WorkspaceFooter host={host} run={selectedRun} error={Boolean(workspace.error)} agentCount={data?.agents.length ?? 0} />
    </main>
    </WorkspaceSplit>
    {host && <Settings open={settings} host={host} onClose={() => setSettings(false)} onSaved={workspace.reset} onDirty={onDirty} />}
    {newTask && <NewTask target={target} task={task} setTarget={setTarget} setTask={setTask} busy={busy} onClose={() => setNewTask(false)} onStart={start} />}
    {resume && <ResumeTask key={resume.id} run={resume} onClose={() => setResume(undefined)} onResume={resumeRun} />}
    {confirmation && <Modal title={confirmation.kind === 'close' ? t('workspace-close-workspace') : t('workspace-stop', { v0: confirmation.agent.name })} description={confirmation.kind === 'close' ? `${confirmation.busy ? t('workspace-running-analysis-will-pause-and-save-before') : ''}${anyDraft ? t('workspace-unsent-messages-and-unsaved-settings-will-be') : ''}` : t('workspace-this-agent-and-its-subtasks-will-stop')} onClose={() => { if (!closing) setConfirmation(undefined); }}>
      {closing && <p className="muted"><StateIcon state="running" />{t('workspace-waiting-for-analysis-and-file-operations-to')}</p>}
      <div className="modal-footer"><button disabled={closing || pending} onClick={() => setConfirmation(undefined)}>{t('workspace-cancel')}</button><button className="danger" disabled={closing || pending} onClick={() => {
        if (confirmation.kind === 'close') void close();
        else { const captured = confirmation; void command(async () => { await api('stop_agent', { epoch: captured.epoch, run: captured.run, agent: captured.agent.id }); setConfirmation(undefined); }); }
      }}>{confirmation.kind === 'close' ? closing ? t('workspace-saving') : t('workspace-save-progress-and-exit') : t('workspace-stop-this-agent')}</button></div>
    </Modal>}
  </div>;
}

export function WorkspaceTabs({ tab, onTab, findingsCount, agentsVisible, showAgentToggle, onToggleAgents }: {
  tab: WorkspaceTab; onTab: (tab: WorkspaceTab) => void; findingsCount: number;
  agentsVisible: boolean; showAgentToggle: boolean; onToggleAgents: () => void;
}) {
  return <nav className="workspace-tabs" aria-label={t('workspace-workspace-views')}>
    <button className={tab === 'conversation' ? 'active' : ''} onClick={() => onTab('conversation')}><MessageSquare size={15} />{t('workspace-analysis-progress')}</button>
    <button className={tab === 'findings' ? 'active' : ''} onClick={() => onTab('findings')}><FileSearch size={15} />{t('workspace-findings-and-evidence')}<span>{findingsCount}</span></button>
    <button className={tab === 'overview' ? 'active' : ''} onClick={() => onTab('overview')}><Activity size={15} />{t('workspace-task-overview')}</button>
    <button className={tab === 'observability' ? 'active' : ''} onClick={() => onTab('observability')}><Gauge size={15} />{t('observability-title')}</button>
    <div className="tab-spacer" />{showAgentToggle && <button className={`agents-toggle ${agentsVisible ? 'enabled' : ''}`} aria-label={t('workspace-toggle-collaboration-panel')} aria-pressed={agentsVisible} onClick={onToggleAgents}><GitBranch size={15} />{t('workspace-collaboration')}</button>}
  </nav>;
}

export function SelectedRunContent({ tab, showAgents, agents, conversation, findings, overview, observability }: {
  tab: WorkspaceTab; showAgents: boolean; agents: ReactNode; conversation: ReactNode; findings: ReactNode; overview: ReactNode; observability: ReactNode;
}) {
  const content = tab === 'conversation' ? conversation : tab === 'findings' ? findings : tab === 'overview' ? overview : observability;
  if (tab === 'observability') return <div className="primary-panel">{content}</div>;
  return <WorkspaceSplit id="agent-inspection" label={t('workspace-resize-collaboration-panel')} visible={showAgents} side={agents}>
    <div className="primary-panel">{content}</div>
  </WorkspaceSplit>;
}

export function Overview({ data, run, onAgent }: { data: Workspace; run: Run; onAgent: (id: string) => void }) {
  return <div className="overview-scroll"><section className="overview-summary"><span className="eyebrow">ANALYSIS WORKSPACE</span><h2>{fileName(run.target_path)}</h2><p className="muted path">{run.target_path}</p><Markdown text={run.task} />
    <div className="metrics"><div><span>Tokens</span><strong>{compact(run.total_tokens)}</strong></div><div><span>{t('workspace-total-cost')}</span><strong>${run.cost_usd.toFixed(4)}</strong></div><div><span>{t('workspace-collaborating-agents')}</span><strong>{data.agents.length}</strong></div><div><span>{t('workspace-findings')}</span><strong>{data.findings.length}</strong></div></div>
    {run.summary && <><h3>{t('workspace-task-summary')}</h3><Markdown text={run.summary} /></>}
  </section><section className="overview-agents"><h3>{t('workspace-collaboration')}</h3>{data.agents.map(agent => <button key={agent.id} onClick={() => onAgent(agent.id)}><Bot size={18} /><div><strong>{agent.name}</strong><p>{agent.task}</p></div><Status value={agent.status} /><ArrowRight size={14} /></button>)}</section>
    {data.notes.length > 0 && <section className="shared-notes"><h3>{t('workspace-shared-analysis-notes')}</h3>{data.notes.map(note => <details key={note.id}><summary>{note.title}</summary><Markdown text={note.body} /></details>)}</section>}</div>;
}
