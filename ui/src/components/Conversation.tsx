import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { AssistantRuntimeProvider, ThreadPrimitive, MessagePrimitive, useExternalStoreRuntime,
  useAuiState, groupPartByType, type ThreadMessageLike } from '@assistant-ui/react';
import { useVirtualizer } from '@tanstack/react-virtual';
import * as Collapsible from '@radix-ui/react-collapsible';
import { ArrowDown, ArrowUp, Bot, ChevronRight, Code2, User } from 'lucide-react';
import { api, errorText } from '../bridge';
import type { Agent, ChatMessage, Host, ToolDisplay, ToolRecord, Workspace } from '../types';
import { Markdown } from './Markdown';
import { labels, StateIcon, Status, terminal } from './Status';

const TranscriptContext = createContext<{ epoch: number; run: string; onAgent: (id: string) => void }>({ epoch: 0, run: '', onAgent: () => {} });
const groupTools = groupPartByType({ 'tool-call': ['group-tools'] });

export function ToolStep({ display }: { display: ToolDisplay }) {
  const context = useContext(TranscriptContext);
  const [open, setOpen] = useState(false);
  const [record, setRecord] = useState<ToolRecord>();
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!open) return;
    let alive = true;
    setLoading(true); setError('');
    void api<ToolRecord>('tool_details', { epoch: context.epoch, run: context.run, id: display.call_id })
      .then(value => { if (alive) setRecord(value); }).catch(error => { if (alive) setError(errorText(error)); })
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [open, display.call_id, display.state, context.epoch, context.run]);
  return <Collapsible.Root open={open} onOpenChange={setOpen} className={`tool-step tool-${display.state}`}>
    <Collapsible.Trigger className="tool-trigger">
      <span className="tool-symbol"><StateIcon state={display.state} /></span>
      <span className="tool-label"><strong>{display.title}</strong>{display.target && <span className="tool-target" title={display.target}>{display.target}</span>}</span>
      <span className="tool-state">{labels[display.state] ?? display.state}</span>
      {display.duration_ms != null && <span className="tool-duration">{(display.duration_ms / 1000).toFixed(1)}s</span>}
      <ChevronRight size={13} className={open ? 'rotate' : ''} />
    </Collapsible.Trigger>
    {display.purpose && <p className="tool-purpose">{display.purpose}</p>}
    {display.summary && <p className="tool-summary">{display.summary}</p>}
    <Collapsible.Content className="tool-details">
      {loading && <p className="muted">正在读取操作详情…</p>}
      {error && <p className="error" role="alert">{error}</p>}
      {record && <><div className="detail-caption"><Code2 size={13} />原始操作详情</div>
        <pre>{JSON.stringify(record.arguments, null, 2)}</pre>
        {record.result != null && <><div className="detail-caption">原始返回</div><pre>{typeof record.result === 'string' ? record.result : JSON.stringify(record.result, null, 2)}</pre></>}
        {record.evidence_id && <p className="muted">证据：<code>{record.evidence_id}</code></p>}</>}
    </Collapsible.Content>
  </Collapsible.Root>;
}
function ToolGroup({ indices }: { indices: readonly number[] }) {
  const content = useAuiState(s => s.message.content);
  const steps = indices.map(index => content[index]).filter(part => part?.type === 'tool-call')
    .map(part => (part.args as unknown as { display: ToolDisplay }).display);
  const working = steps.some(step => ['running', 'queued'].includes(step.state));
  const failures = steps.filter(step => ['failed', 'uncertain', 'interrupted'].includes(step.state)).length;
  const [expanded, setExpanded] = useState(false);
  const open = expanded || working || indices.length <= 4;
  return <Collapsible.Root open={open} onOpenChange={setExpanded} className="tool-group">
    <Collapsible.Trigger className="tool-group-heading" disabled={working || indices.length <= 4}>
      <span><StateIcon state={working ? 'running' : failures ? 'failed' : 'completed'} />
        {working ? '正在执行操作' : `${indices.length} 项操作`}{failures > 0 ? ` · ${failures} 项需关注` : ''}</span>
      {indices.length > 4 && !working && <ChevronRight size={14} className={open ? 'rotate' : ''} />}
    </Collapsible.Trigger>
    <Collapsible.Content>{(expanded ? steps : steps.slice(-4)).map(step => <ToolStep key={step.call_id} display={step} />)}
      {!expanded && steps.length > 4 && <button className="earlier" onClick={() => setExpanded(true)}>展开全部 {steps.length} 项操作</button>}
    </Collapsible.Content>
  </Collapsible.Root>;
}
function Message() {
  const message = useAuiState(s => s.message);
  const context = useContext(TranscriptContext);
  const custom = message.metadata.custom as { author?: string; agent?: string; delivery?: string };
  const user = message.role === 'user';
  return <MessagePrimitive.Root className={`message ${user ? 'message-user' : 'message-assistant'}`}>
    <div className="message-heading"><span className={`avatar ${user ? 'avatar-user' : ''}`}>{user ? <User size={15} /> : <Bot size={16} />}</span>
      <button className="author" onClick={() => { if (custom.agent) context.onAgent(custom.agent); }} disabled={!custom.agent}>{custom.author ?? (user ? '你' : 'Agent')}</button>
      <time dateTime={message.createdAt.toISOString()}>{message.createdAt.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time>
      {custom.delivery && <span className={`delivery delivery-${custom.delivery}`}>{custom.delivery === 'queued' ? '待接收' : labels[custom.delivery] ?? custom.delivery}</span>}
    </div>
    <div className="message-content"><MessagePrimitive.GroupedParts groupBy={groupTools} indicator="never">
      {({ part }) => {
        switch (part.type) {
          case 'text': return <Markdown text={part.text} />;
          case 'group-tools': return <ToolGroup indices={part.indices} />;
          case 'tool-call': return <ToolStep display={(part.result as { display?: ToolDisplay } | undefined)?.display ?? (part.args as unknown as { display: ToolDisplay }).display} />;
          default: return <></>;
        }
      }}
    </MessagePrimitive.GroupedParts></div>
  </MessagePrimitive.Root>;
}
const messageComponents = { Message };
export function convertMessage(message: ChatMessage): ThreadMessageLike {
  return { id: message.id, role: message.role, createdAt: new Date(message.created_at),
    status: message.role === 'assistant' ? { type: 'complete', reason: 'stop' } : undefined,
    content: message.content as unknown as ThreadMessageLike['content'],
    metadata: { custom: { author: message.author, agent: message.agent_id, delivery: message.delivery } } };
}
function Transcript({ data, loadEarlier, limit }: { data: Workspace; loadEarlier: () => void; limit: number }) {
  const messages = useAuiState(s => s.thread.messages);
  const viewport = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const anchor = useRef<{ id: string } | undefined>(undefined);
  const virtual = useVirtualizer({ count: messages.length, getScrollElement: () => viewport.current,
    estimateSize: () => 200, overscan: 6, getItemKey: index => messages[index].id });
  useLayoutEffect(() => {
    if (pinned.current && messages.length) virtual.scrollToIndex(messages.length - 1, { align: 'end' });
    else if (anchor.current) {
      const index = messages.findIndex(message => message.id === anchor.current!.id);
      if (index >= 0) virtual.scrollToIndex(index, { align: 'start' });
      anchor.current = undefined;
    }
  }, [messages, virtual]);
  return <ThreadPrimitive.Viewport ref={viewport} className="transcript" autoScroll={false}
    scrollToBottomOnInitialize={false} scrollToBottomOnRunStart={false} onScroll={() => {
      const element = viewport.current;
      if (element) pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 120;
    }}>
    <div className="transcript-inner">
      {data.has_earlier && <button className="earlier" disabled={limit >= 10_000} onClick={() => {
        pinned.current = false;
        const first = virtual.getVirtualItems()[0];
        if (first) anchor.current = { id: messages[first.index].id };
        loadEarlier();
      }}>{limit >= 10_000 ? '已展示最近 10,000 条事件，完整记录可导出报告' : '加载更早的进展'}</button>}
      {!messages.length && <div className="empty-conversation"><Bot size={28} /><h3>工作进展将在这里展开</h3><p>Agent 的公开说明、工具操作和结果会按时间展示。</p></div>}
      <div style={{ height: virtual.getTotalSize(), position: 'relative', width: '100%' }}>
        {virtual.getVirtualItems().map(item => <div key={item.key} ref={virtual.measureElement} data-index={item.index}
          className="virtual-message" style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}>
          {messages[item.index] && <ThreadPrimitive.Unstable_MessageById messageId={messages[item.index].id} components={messageComponents} />}
        </div>)}
      </div>
    </div>
    <button className="jump-latest" aria-label="回到最新进展" onClick={() => { pinned.current = true; if (messages.length) virtual.scrollToIndex(messages.length - 1, { align: 'end' }); }}><ArrowDown size={15} />最新进展</button>
  </ThreadPrimitive.Viewport>;
}
export function Conversation({ data, host, agent, onAgent, loadEarlier, limit, composer }: {
  data: Workspace; host: Host; agent?: Agent; onAgent: (id: string) => void;
  loadEarlier: () => void; limit: number; composer: ReactNode;
}) {
  const runtime = useExternalStoreRuntime({ messages: data.transcript, convertMessage,
    isRunning: host.execution?.run_id === data.selected,
    onNew: async () => { throw new Error('请使用下方操作消息输入框。'); }, isSendDisabled: true });
  return <TranscriptContext.Provider value={{ epoch: host.epoch, run: data.selected ?? '', onAgent }}>
    <AssistantRuntimeProvider runtime={runtime}><ThreadPrimitive.Root className="conversation">
      {agent && <div className="agent-context"><Bot size={17} /><div><strong>{agent.name}</strong><span>{agent.task}</span></div><Status value={agent.status} /></div>}
      <Transcript data={data} loadEarlier={loadEarlier} limit={limit} />
      {composer}
    </ThreadPrimitive.Root></AssistantRuntimeProvider>
  </TranscriptContext.Provider>;
}
export interface Draft { text: string; followUp: boolean }
export function Composer({ target, draft, onDraft, send, busy }: {
  target?: Agent; draft: Draft; onDraft: (draft: Draft) => void;
  send: (draft: Draft, target: Agent) => Promise<void>; busy: boolean;
}) {
  const [error, setError] = useState('');
  const [sending, setSending] = useState(false);
  const submit = async () => {
    if (!target || !draft.text.trim() || sending || busy) return;
    if (terminal(target.status) && !draft.followUp) { setError('该 Agent 已结束。勾选“安排后续任务”后发送。'); return; }
    const submitted = { ...draft };
    setSending(true); setError('');
    try {
      await send(submitted, target);
    } catch (error) { setError(errorText(error)); }
    finally { setSending(false); }
  };
  return <div className="composer-wrap"><div className="composer">
    <textarea aria-label="发送给 Agent 的消息" placeholder={target ? `补充信息或调整 ${target.name} 的工作…` : '选择任务后可向 Agent 发送消息'}
      value={draft.text} disabled={!target} rows={3} onChange={event => onDraft({ ...draft, text: event.target.value })}
      onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); void submit(); } }} />
    <div className="composer-controls"><div className="composer-target"><Bot size={13} /><span>{target?.name ?? '等待选择任务'}</span></div>
      <label className="checkbox"><input type="checkbox" checked={draft.followUp} onChange={event => onDraft({ ...draft, followUp: event.target.checked })} />安排后续任务</label>
      <button className="send-button" aria-label="发送消息" disabled={!target || !draft.text.trim() || sending || busy} onClick={() => { void submit(); }}>{sending ? <StateIcon state="running" /> : <ArrowUp size={17} />}</button>
    </div>
  </div>{error ? <p className="error composer-hint" role="alert">{error}</p> : <p className="composer-hint">{target && terminal(target.status) ? '后续任务会入队，恢复分析后执行。' : '消息按 Agent 代次投递；接收状态会显示在会话中。'}<span>⌘ / Ctrl + Enter 发送</span></p>}</div>;
}
