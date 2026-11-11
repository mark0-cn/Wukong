import { t, text, message as formatMessage, useLocale, currentLocale } from '../i18n';
import { createContext, useContext, useMemo, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { AssistantRuntimeProvider, ThreadPrimitive, MessagePrimitive, useExternalStoreRuntime,
  useAuiState, groupPartByType, type ThreadMessageLike } from '@assistant-ui/react';
import { useVirtualizer } from '@tanstack/react-virtual';
import * as Collapsible from '@radix-ui/react-collapsible';
import { ArrowDown, ArrowUp, Bot, ChevronRight, Code2, User, Maximize2, Minimize2 } from 'lucide-react';
import { api, errorText, errorValue } from '../bridge';
import type { Agent, ChatMessage, Host, ToolDisplay, ToolRecord, Workspace } from '../types';
import { CodeBlock, Markdown } from './Markdown';
import { labels, StateIcon, Status, terminal } from './Status';
import { JobFacts, ToolDuration } from './ToolActivity';

const TranscriptContext = createContext<{ epoch: number; run: string; onAgent: (id: string) => void }>({ epoch: 0, run: '', onAgent: () => {} });
const groupTools = groupPartByType({ 'tool-call': ['group-tools'] });

export function ToolStep({ display }: { display: ToolDisplay }) {
  useLocale();
  const context = useContext(TranscriptContext);
  const [open, setOpen] = useState(false);
  const [record, setRecord] = useState<ToolRecord>();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!open) return;
    let alive = true;
    setLoading(true); setError('');
    void api('tool_details', { epoch: context.epoch, run: context.run, id: display.call_id })
      .then(value => { if (alive) setRecord(value); }).catch(error => { if (alive) setError(errorValue(error)); })
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [open, display.call_id, display.state, context.epoch, context.run]);
  return <Collapsible.Root open={open} onOpenChange={setOpen} className={`tool-step tool-${display.state}`}>
    <Collapsible.Trigger className="tool-trigger">
      <span className="tool-symbol"><StateIcon state={display.state} /></span>
      <span className="tool-label"><strong>{text(display.title)}</strong>{display.target && <span className="tool-target" title={display.target}>{display.target}</span>}</span>
      {display.state === 'completed' && !open && display.summary && <span className="tool-inline-summary" title={formatMessage(display.summary)}>{formatMessage(display.summary)}</span>}
      <span className="tool-state">{labels[display.state] ?? display.state}</span>
      <ToolDuration display={display} />
      <ChevronRight size={13} className={open ? 'rotate' : ''} />
    </Collapsible.Trigger>
    {(open || display.state !== 'completed') && display.purpose && <p className="tool-purpose">{display.purpose}</p>}
    {(open || display.state !== 'completed') && display.summary && <p className="tool-summary">{formatMessage(display.summary)}</p>}
    <JobFacts display={display} />
    {display.preview && <div className="tool-code-preview"><CodeBlock text={display.preview.text} language={display.preview.language} />
      <p className="muted">{t('workspace-evidence')}<code>{display.preview.evidence_id}</code>{display.preview.truncated && <> · {t('progress-preview-truncated')}</>}</p></div>}
    <Collapsible.Content className="tool-details">
      {loading && <p className="muted">{t('workspace-loading-operation-details')}</p>}
      {error && <p className="error" role="alert">{errorText(error)}</p>}
      {record && <><div className="detail-caption"><Code2 size={13} />{t('workspace-raw-operation-details')}</div>
        <p className="muted">{t('progress-invocation-state')}: {labels[record.state] ?? record.state}
          {display.job && <> · {t('progress-job-identity', { id: display.job.id })} <code>{display.job.state}</code></>}</p>
        <pre>{JSON.stringify(record.arguments, null, 2)}</pre>
        {record.result != null && <><div className="detail-caption">{t('workspace-raw-result')}</div><pre>{typeof record.result === 'string' ? record.result : JSON.stringify(record.result, null, 2)}</pre></>}
        {record.evidence_id && <p className="muted">{t('workspace-evidence')}<code>{record.evidence_id}</code></p>}</>}
    </Collapsible.Content>
  </Collapsible.Root>;
}
function ToolGroup({ indices }: { indices: readonly number[] }) {
  useLocale();
  const content = useAuiState(s => s.message.content);
  const steps = indices.map(index => content[index]).filter(part => part?.type === 'tool-call')
    .map(part => (part.args as unknown as { display: ToolDisplay }).display);
  const working = steps.some(step => ['running', 'queued'].includes(step.state));
  const failures = steps.filter(step => ['failed', 'uncertain', 'interrupted'].includes(step.state)).length;
  const [expanded, setExpanded] = useState(false);
  const salient = steps.filter(step => step.state !== 'completed');
  const open = expanded || salient.length > 0 || steps.length <= 3;
  return <Collapsible.Root open={open} onOpenChange={setExpanded} className="tool-group">
    <Collapsible.Trigger className="tool-group-heading" disabled={steps.length <= 3}>
      <span><StateIcon state={working ? 'running' : failures ? 'failed' : 'completed'} />
        {working ? t('workspace-running-operations') : t('workspace-operations', { v0: indices.length })}{failures > 0 ? ` · ${t('workspace-need-attention', { v0: failures })}` : ''}</span>
      {steps.length > 3 && <ChevronRight size={14} className={open ? 'rotate' : ''} />}
    </Collapsible.Trigger>
    <Collapsible.Content>{(expanded || steps.length <= 3 ? steps : salient).map(step => <ToolStep key={step.call_id} display={step} />)}
      {!expanded && steps.length > 3 && <button className="earlier" onClick={() => setExpanded(true)}>{t('workspace-expand-all-operations', { count: steps.length })}</button>}
    </Collapsible.Content>
  </Collapsible.Root>;
}
function Message() {
  useLocale();
  const message = useAuiState(s => s.message);
  const context = useContext(TranscriptContext);
  const custom = message.metadata.custom as { author?: string; agent?: string; delivery?: string; state?: ChatMessage['state']; truncated?: boolean };
  const user = message.role === 'user';
  return <MessagePrimitive.Root className={`message ${user ? 'message-user' : 'message-assistant'}`}>
    <div className="message-heading"><span className={`avatar ${user ? 'avatar-user' : ''}`}>{user ? <User size={15} /> : <Bot size={16} />}</span>
      <button className="author" onClick={() => { if (custom.agent) context.onAgent(custom.agent); }} disabled={!custom.agent}>{custom.author ?? (user ? t('workspace-you') : 'Agent')}</button>
      <time dateTime={message.createdAt.toISOString()}>{message.createdAt.toLocaleTimeString(currentLocale(), { hour: '2-digit', minute: '2-digit' })}</time>
      {custom.delivery && <span className={`delivery delivery-${custom.delivery}`}>{custom.delivery === 'queued' ? t('workspace-awaiting-delivery') : labels[custom.delivery] ?? custom.delivery}</span>}
      {custom.state === 'streaming' && <span className="delivery" role="status">{t('progress-text-streaming')}</span>}
      {custom.state === 'interrupted' && <span className="delivery delivery-stale">{t('progress-text-interrupted')}</span>}
      {custom.truncated && <span className="delivery">{t('progress-text-truncated')}</span>}
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
    status: message.role === 'assistant' ? message.state === 'streaming' ? { type: 'running' }
      : message.state === 'interrupted' ? { type: 'incomplete', reason: 'cancelled' }
      : { type: 'complete', reason: 'stop' } : undefined,
    content: message.content.map(part => part.type === 'text' ? { ...part, text: formatMessage(part.text) } : part) as unknown as ThreadMessageLike['content'],
    metadata: { custom: { author: formatMessage(message.author), agent: message.agent_id, delivery: message.delivery, state: message.state, truncated: message.truncated } } };
}
function Transcript({ data, loadEarlier, limit }: { data: Workspace; loadEarlier: () => void; limit: number }) {
  useLocale();
  const messages = useAuiState(s => s.thread.messages);
  const viewport = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [showLatest, setShowLatest] = useState(false);
  const [unread, setUnread] = useState(0);
  const previousTail = useRef<string | undefined>(undefined);
  const anchor = useRef<{ id: string } | undefined>(undefined);
  const virtual = useVirtualizer({ count: messages.length, getScrollElement: () => viewport.current,
    estimateSize: () => 200, overscan: 6, getItemKey: index => messages[index].id });
  useLayoutEffect(() => {
    const tail = messages.at(-1)?.id;
    if (!pinned.current && previousTail.current && tail !== previousTail.current) {
      const index = messages.findIndex(item => item.id === previousTail.current);
      if (index >= 0) setUnread(value => value + messages.length - index - 1);
    }
    previousTail.current = tail;
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
      if (element) {
        pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
        setShowLatest(!pinned.current);
        if (pinned.current) setUnread(0);
      }
    }}>
    <div className="transcript-inner">
      {data.has_earlier && <button className="earlier" disabled={limit >= 10_000} onClick={() => {
        pinned.current = false;
        const first = virtual.getVirtualItems()[0];
        if (first) anchor.current = { id: messages[first.index].id };
        loadEarlier();
      }}>{limit >= 10_000 ? t('workspace-showing-the-latest-10-000-events-export') : t('workspace-load-earlier-progress')}</button>}
      {!messages.length && <div className="empty-conversation"><Bot size={28} /><h3>{t('workspace-work-unfolds-here')}</h3><p>{t('workspace-public-agent-updates-tool-operations-and-results')}</p></div>}
      <div style={{ height: virtual.getTotalSize(), position: 'relative', width: '100%' }}>
        {virtual.getVirtualItems().map(item => <div key={item.key} ref={virtual.measureElement} data-index={item.index}
          className="virtual-message" style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}>
          {messages[item.index] && <ThreadPrimitive.Unstable_MessageById messageId={messages[item.index].id} components={messageComponents} />}
        </div>)}
      </div>
    </div>
    {showLatest && <button className="jump-latest" aria-label={t('workspace-jump-to-latest-progress')} onClick={() => { pinned.current = true; setShowLatest(false); setUnread(0); if (messages.length) virtual.scrollToIndex(messages.length - 1, { align: 'end' }); }}><ArrowDown size={15} />{unread ? t('ui-new-messages', { count: unread }) : t('workspace-latest-progress')}</button>}
  </ThreadPrimitive.Viewport>;
}
export function Conversation({ data, host, agent, onAgent, loadEarlier, limit, composer }: {
  data: Workspace; host: Host; agent?: Agent; onAgent: (id: string) => void;
  loadEarlier: () => void; limit: number; composer: ReactNode;
}) {
  useLocale();
  const locale = currentLocale();
  const messages = useMemo(() => data.transcript.map(convertMessage), [data.transcript, locale]);
  const runtime = useExternalStoreRuntime({ messages, convertMessage: (message: ThreadMessageLike) => message,
    isRunning: host.execution?.run_id === data.selected,
    onNew: async () => { throw new Error(t('workspace-use-the-message-composer-below')); }, isSendDisabled: true });
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
  useLocale();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [sending, setSending] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    if (!input.current) return;
    input.current.style.height = 'auto';
    input.current.style.height = `${expanded ? 180 : Math.min(140, Math.max(52, input.current.scrollHeight))}px`;
  }, [draft.text, expanded]);
  const submit = async () => {
    if (!target || !draft.text.trim() || sending || busy) return;
    if (terminal(target.status) && !draft.followUp) { setError(t('workspace-this-agent-has-finished-select-schedule-follow')); return; }
    const submitted = { ...draft };
    setSending(true); setError('');
    try {
      await send(submitted, target);
    } catch (error) { setError(errorValue(error)); }
    finally { setSending(false); }
  };
  return <div className="composer-wrap"><div className="composer">
    <textarea ref={input} aria-label={t('workspace-message-to-agent')} placeholder={target ? t('workspace-add-information-or-adjust-s-work', { v0: target.name }) : t('workspace-select-a-task-to-message-an-agent')}
      value={draft.text} disabled={!target} rows={2} onChange={event => onDraft({ ...draft, text: event.target.value })}
      onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); void submit(); } }} />
    <div className="composer-controls"><div className="composer-target"><Bot size={13} /><span title={target?.name}>{t('ui-recipient')}{' '}{target?.name ?? t('workspace-select-a-task-first')}</span></div>
      <select className="send-mode" aria-label={t('ui-send-mode')} value={draft.followUp ? 'follow_up' : 'information'} onChange={event => { setError(''); onDraft({ ...draft, followUp: event.target.value === 'follow_up' }); }}>
        <option value="information">{t('ui-send-information')}</option><option value="follow_up">{t('ui-send-task')}</option>
      </select>
      <button className="icon-button composer-expand" aria-label={expanded ? t('ui-collapse-input') : t('ui-expand-input')} aria-pressed={expanded} onClick={() => setExpanded(value => !value)}>{expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</button>
      <button className="send-button" aria-label={t('workspace-send-message')} disabled={!target || !draft.text.trim() || sending || busy} onClick={() => { void submit(); }}>{sending ? <StateIcon state="running" /> : <ArrowUp size={17} />}</button>
    </div>
  </div>{error ? <p className="error composer-hint" role="alert">{errorText(error)}</p> : <p className="composer-hint">{target && terminal(target.status) ? t('ui-ended-recipient') : draft.followUp ? t('ui-append-help') : ''}<span>{t('workspace-ctrl-enter-to-send')}</span></p>}</div>;
}
