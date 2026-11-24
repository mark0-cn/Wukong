import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { api, openLink } from '../../shared/api/bridge';
import type {
  ObservabilityLocalActivity, ObservabilityLogBatch, ObservabilityMetricBatch, ObservabilityRunSnapshot,
  ObservabilitySignal, ObservabilityStatus, ObservabilityTrace, ObservabilityTraceList, Commands,
} from '../../shared/api/commands';
import { Observability } from './Observability';
import { ObservabilitySettings } from '../settings/ObservabilitySettings';
import { editedForm } from '../settings/transport';
import type { Json } from '../../shared/api/types';

const timelineMock = vi.hoisted(() => ({
  items: [] as { content?: HTMLElement }[],
  groups: [] as unknown[],
  selectCallbacks: [] as ((properties: { items?: (string | number)[] }) => void)[],
  destroyed: 0,
}));
const chartMock = vi.hoisted(() => ({ init: vi.fn(), setOption: vi.fn(), dispose: vi.fn(), resize: vi.fn() }));
vi.mock('../../shared/api/bridge', () => ({
  api: vi.fn(), openLink: vi.fn(), errorText: (error: unknown) => error instanceof Error ? error.message : String(error), errorValue: String,
}));
vi.mock('echarts', () => ({ init: chartMock.init }));
vi.mock('vis-data/peer', () => ({ DataSet: class<T> {
  items: T[];
  constructor(items: T[]) { this.items = items; }
  get length() { return this.items.length; }
} }));
vi.mock('vis-timeline/peer', () => ({ Timeline: class {
  constructor(_element: HTMLElement, items: { items: { content?: HTMLElement }[] }, groups: { items: unknown[] }) {
    timelineMock.items = items.items; timelineMock.groups = groups.items;
  }
  on(event: string, callback: (properties: { items?: (string | number)[] }) => void) { if (event === 'select') timelineMock.selectCallbacks.push(callback); }
  fit() {}
  setWindow() {}
  setSelection() {}
  destroy() { timelineMock.destroyed += 1; }
} }));
vi.mock('vis-timeline/styles/vis-timeline-graph2d.min.css', () => ({}));
vi.mock('@tanstack/react-virtual', () => ({ useVirtualizer: ({ count, getItemKey }: { count: number; getItemKey: (index: number) => string | number }) => ({
  getTotalSize: () => count * 82,
  getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, key: getItemKey(index), start: index * 82, size: 82 })),
  measureElement: () => {},
}) }));

const range = { start: '2026-11-23T00:00:00Z', end: '2026-11-23T01:00:00Z' };
const traceList: ObservabilityTraceList = { run_id: 'run', next_cursor: null, traces: [{
  trace_id: 'trace-1', root_service_name: 'wukong', root_trace_name: 'root operation', start_time: range.start, duration_ms: 25, span_count: 2,
}] };
const logBatch: ObservabilityLogBatch = { run_id: 'run', trace_id: null, next_cursor: null, entries: [{
  timestamp: range.start, timestamp_nanos: '1', line: 'plain log line', line_truncated: false,
  labels: { service_name: 'wukong', level: 'info', target: 'runtime' }, attributes: {},
}] };
const metricBatch: ObservabilityMetricBatch = { metric: 'runs_started', expression: 'sum(runs_started)', scope: 'service', step_seconds: 30,
  series: [{ labels: { service_name: 'wukong' }, points: [{ timestamp: range.start, value: 3 }] }] };
const status: ObservabilityStatus = {
  capabilities: {
    tempo: { configured: true, origin: 'https://tempo.example', credential_available: false },
    loki: { configured: true, origin: 'https://loki.example', credential_available: false },
    prometheus: { configured: true, origin: 'https://prometheus.example', credential_available: false },
    grafana: { configured: true, origin: 'https://grafana.example', credential_available: false },
  },
  exporter: { activated: true, traces: true, metrics: true, logs: false, local_jsonl: true, worker_relay: true,
    worker_relay_status: { traces: { healthy: true, last_error: null }, logs: { healthy: null, last_error: null } }, last_error: null },
  queried_at: range.end,
};
function local(run: string, summary = 'local summary'): ObservabilityLocalActivity {
  return { activity_kind: 'local_business_activity', run: { id: run, status: 'running', root_id: 'root', created_at: range.start, updated_at: range.end,
    total_tokens: 42, cost_usd: 0.25, summary },
    agents: [{ id: 'root', parent_id: null, name: 'Lead', role: 'root', status: 'running', generation: 1, turns: 2, total_tokens: 42, cost_usd: 0.25, created_at: range.start, updated_at: range.end }],
    events: [{ seq: 1, agent_id: 'root', kind: 'message', created_at: range.start, data: { text: `${run} event` }, data_truncated: false }],
    tools: { total: 1, prepared: 0, running: 1, completed: 0, failed: 0, uncertain: 0, side_effecting: 1 },
    resources: { jobs: 1, active_jobs: 1, terminal_jobs: 0, evidence_pending: 0 }, next_cursor: '1', has_more: true,
    partial: false, truncated: false, stale: false, queried_at: range.end };
}
function signal<T>(data: T | null, overrides: Partial<ObservabilitySignal<T>> = {}): ObservabilitySignal<T> {
  return { configured: true, partial: false, truncated: false, stale: false, queried_at: range.end, error: null, data, ...overrides };
}
function snapshot(run = 'run', overrides: Partial<ObservabilityRunSnapshot> = {}): ObservabilityRunSnapshot {
  return { local: local(run, `${run} local summary`), traces: signal({ ...traceList, run_id: run }),
    logs: signal({ ...logBatch, run_id: run }), metrics: signal(metricBatch), ...overrides };
}
const unconfigured = <T,>() => signal<T>(null, { configured: false });
let runHandler: (args: Commands['observability_run']['args']) => Promise<ObservabilityRunSnapshot>;
let statusHandler: () => Promise<ObservabilityStatus>;
let localPageHandler: (cursor: string | null) => Promise<ObservabilityLocalActivity>;
let traceHandler: () => Promise<ObservabilitySignal<ObservabilityTrace>>;
let logsQueryHandler: (args: Commands['observability_logs']['args']) => Promise<ObservabilitySignal<ObservabilityLogBatch>>;
let tracesPageHandler: (args: Commands['observability_traces']['args']) => Promise<ObservabilitySignal<ObservabilityTraceList>>;
let grafanaHandler: () => Promise<string | null>;

beforeEach(() => {
  vi.mocked(api).mockReset(); vi.mocked(openLink).mockReset(); vi.mocked(openLink).mockResolvedValue(undefined);
  chartMock.init.mockReset(); chartMock.setOption.mockReset(); chartMock.dispose.mockReset(); chartMock.resize.mockReset();
  chartMock.init.mockReturnValue({ setOption: chartMock.setOption, dispose: chartMock.dispose, resize: chartMock.resize });
  timelineMock.items = []; timelineMock.groups = []; timelineMock.selectCallbacks = []; timelineMock.destroyed = 0;
  Object.defineProperty(window, 'matchMedia', { configurable: true, value: vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }) });
  statusHandler = () => Promise.resolve(status);
  localPageHandler = () => Promise.reject(new Error('unexpected local page query'));
  runHandler = ({ request }) => Promise.resolve(snapshot(request.run));
  traceHandler = () => Promise.reject(new Error('unexpected trace query'));
  logsQueryHandler = () => Promise.reject(new Error('unexpected logs query'));
  tracesPageHandler = () => Promise.reject(new Error('unexpected traces page query'));
  grafanaHandler = () => Promise.resolve(null);
  vi.mocked(api).mockImplementation(((command: string, args: unknown) => {
    if (command === 'observability_status') return statusHandler();
    if (command === 'observability_run') return runHandler(args as Commands['observability_run']['args']);
    if (command === 'observability_trace') return traceHandler();
    if (command === 'observability_local_activity') return localPageHandler((args as Commands['observability_local_activity']['args']).cursor);
    if (command === 'observability_logs') return logsQueryHandler(args as Commands['observability_logs']['args']);
    if (command === 'observability_traces') return tracesPageHandler(args as Commands['observability_traces']['args']);
    if (command === 'observability_grafana_link') return grafanaHandler();
    return Promise.reject(new Error(`Unexpected command: ${command}`));
  }) as never);
});

it('shows real local Store activity when every external backend is unconfigured', async () => {
  runHandler = () => Promise.resolve(snapshot('run', { traces: unconfigured(), logs: unconfigured(), metrics: unconfigured() }));
  statusHandler = () => Promise.resolve({ ...status, capabilities: {
    tempo: { configured: false, origin: null, credential_available: false }, loki: { configured: false, origin: null, credential_available: false },
    prometheus: { configured: false, origin: null, credential_available: false }, grafana: { configured: false, origin: null, credential_available: false },
  } });
  render(<Observability epoch={1} run="run" runStatus="running" />);
  await screen.findByText('run local summary');
  expect(screen.getByText(/run event/)).toBeTruthy();
  expect(screen.getAllByText('未配置')).toHaveLength(4);
  expect(screen.getByText('Worker 中继: 已启用')).toBeTruthy();
  const ranges = screen.getByLabelText('时间范围');
  expect(within(ranges).getAllByRole('option')).toHaveLength(4);
  expect(within(ranges).queryByRole('option', { name: '最近 7 天' })).toBeNull();
  expect(within(ranges).getByRole('option', { name: '最近 24 小时' })).toBeTruthy();
  expect(screen.getByRole('list', { name: '本地事件' }).tabIndex).toBe(0);
});

it('keeps backend errors independent and retains prior metric and trace data as stale', async () => {
  let first = true;
  runHandler = () => {
    if (first) { first = false; return Promise.resolve(snapshot('run', {
      logs: signal<ObservabilityLogBatch>(null, { partial: true, error: { kind: 'http', message: 'loki down', status: 503 } }),
    })); }
    return Promise.resolve(snapshot('run'));
  };
  render(<Observability epoch={1} run="run" runStatus="running" />);
  await screen.findByText('run local summary');
  fireEvent.click(screen.getByRole('button', { name: 'Traces', }));
  expect(await screen.findByText('root operation')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: '日志', }));
  expect(await screen.findByText(/loki down/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: '指标', }));
  await waitFor(() => expect(chartMock.init).toHaveBeenCalled());
  runHandler = () => Promise.resolve(snapshot('run', {
    traces: signal<ObservabilityTraceList>(null, { partial: true, error: { kind: 'timeout', message: 'tempo failed' } }),
    metrics: signal<ObservabilityMetricBatch>(null, { partial: true, error: { kind: 'http', message: 'metrics failed' } }),
    logs: signal(logBatch),
  }));
  fireEvent.click(screen.getByRole('button', { name: '刷新可观察性数据' }));
  expect(await screen.findByText(/metrics failed/)).toBeTruthy();
  expect(screen.getAllByText('数据已过期：显示此前接受的数据').length).toBeGreaterThan(0);
  expect(chartMock.dispose).not.toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole('button', { name: 'Traces', }));
  expect(await screen.findByText(/tempo failed/)).toBeTruthy();
  expect(screen.getByText('root operation')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: '日志', }));
  expect(await screen.findByText('plain log line')).toBeTruthy();
  expect(screen.queryByText(/loki down/)).toBeNull();
});

it('appends local events by sequence and clears the cursor on the final page', async () => {
  localPageHandler = cursor => {
    expect(cursor).toBe('1');
    const last = local('run');
    last.events = [{ seq: 2, agent_id: 'root', kind: 'completed', created_at: range.end, data: { text: 'second event' }, data_truncated: false }];
    last.next_cursor = null; last.has_more = false; last.truncated = false;
    return Promise.resolve(last);
  };
  render(<Observability epoch={1} run="run" runStatus="running" />);
  fireEvent.click(await screen.findByRole('button', { name: '加载更新事件' }));
  expect(await screen.findByText(/second event/)).toBeTruthy();
  expect(screen.getByText('#1')).toBeTruthy();
  expect(screen.getByText('#2')).toBeTruthy();
  expect(screen.queryByRole('button', { name: '加载更新事件' })).toBeNull();
  const refreshed = local('run', 'refreshed local summary');
  refreshed.events = [{ seq: 3, agent_id: 'root', kind: 'fresh', created_at: range.end, data: { text: 'fresh first page' }, data_truncated: false }];
  refreshed.next_cursor = '3'; refreshed.has_more = true;
  runHandler = () => Promise.resolve(snapshot('run', { local: refreshed }));
  fireEvent.click(screen.getByRole('button', { name: '刷新可观察性数据' }));
  await screen.findByText('refreshed local summary');
  expect(screen.queryByText('#1')).toBeNull();
  expect(screen.queryByText('#2')).toBeNull();
  expect(screen.getByText('#3')).toBeTruthy();
  expect(screen.getByRole('button', { name: '加载更新事件' })).toBeTruthy();
});

it('does not allow a late run and epoch response to replace the new run', async () => {
  let resolveOld!: (value: ObservabilityRunSnapshot) => void;
  runHandler = ({ epoch, request }) => epoch === 1
    ? new Promise(resolve => { resolveOld = resolve; }) : Promise.resolve(snapshot(request.run));
  const view = render(<Observability epoch={1} run="old" runStatus="running" />);
  view.rerender(<Observability epoch={2} run="new" runStatus="completed" />);
  await screen.findByText('new local summary');
  await act(async () => resolveOld(snapshot('old', { local: local('old', 'old late summary') })));
  expect(screen.queryByText('old late summary')).toBeNull();
  expect(screen.getByText('new local summary')).toBeTruthy();
});

it('queries a selected trace, renders the vis hierarchy, and exposes safe span properties', async () => {
  const trace: ObservabilityTrace = { trace_id: 'trace-1', spans: [
    { trace_id: 'trace-1', span_id: 'span-1', parent_span_id: null, name: 'parent', start_time: range.start, end_time: range.end,
      duration_nanos: 1000, status: 'ok', attributes: {}, resource: { service: 'wukong' } },
    { trace_id: 'trace-1', span_id: 'span-2', parent_span_id: 'span-1', name: '<b>child</b>', start_time: range.start, end_time: range.end,
      duration_nanos: 500, status: 'error', attributes: { unsafe: '<b>unsafe</b>' }, resource: {} },
  ] };
  traceHandler = () => Promise.resolve(signal(trace));
  render(<Observability epoch={1} run="run" runStatus="running" />);
  fireEvent.click(screen.getByRole('button', { name: 'Traces', }));
  const traceButton = await screen.findByText('root operation');
  fireEvent.click(traceButton.closest('button')!);
  await waitFor(() => expect(timelineMock.items).toHaveLength(2));
  expect(timelineMock.items[1].content?.textContent).toBe('<b>child</b>');
  act(() => timelineMock.selectCallbacks.at(-1)?.({ items: ['span-2'] }));
  expect(await screen.findByText('<b>unsafe</b>')).toBeTruthy();
  expect(document.querySelector('.observability-span-details b')).toBeNull();
  expect(vi.mocked(api<'observability_trace'>)).toHaveBeenCalledWith('observability_trace', { epoch: 1, run: 'run', traceId: 'trace-1' });
});

it('renders log lines as text rather than injected HTML', async () => {
  runHandler = () => Promise.resolve(snapshot('run', { logs: signal({ ...logBatch, entries: [{ ...logBatch.entries[0], line: '<img src=x onerror=alert(1)>' }] }) }));
  render(<Observability epoch={1} run="run" runStatus="running" />);
  fireEvent.click(screen.getByRole('button', { name: '日志', }));
  expect(await screen.findByText('<img src=x onerror=alert(1)>')).toBeTruthy();
  expect(document.querySelector('.observability-log-list img')).toBeNull();
  expect(screen.getByRole('list', { name: '日志条目' }).tabIndex).toBe(0);
});

it('uses the fine-grained native logs query for a trace filter', async () => {
  logsQueryHandler = args => {
    expect(args.traceId).toBe('trace-filter');
    return Promise.resolve(signal({ ...logBatch, trace_id: 'trace-filter', entries: [{ ...logBatch.entries[0], line: 'filtered native line' }] }));
  };
  render(<Observability epoch={1} run="run" runStatus="running" />);
  await screen.findByText('run local summary');
  fireEvent.click(screen.getByRole('button', { name: '日志' }));
  fireEvent.change(screen.getByLabelText('Trace ID'), { target: { value: 'trace-filter' } });
  fireEvent.click(screen.getByRole('button', { name: '应用过滤' }));
  expect(await screen.findByText('filtered native line')).toBeTruthy();
  expect(screen.queryByText('plain log line')).toBeNull();
  expect(api).toHaveBeenCalledWith('observability_logs', { epoch: 1, run: 'run', traceId: 'trace-filter', request: {
    range: { start: expect.any(String), end: expect.any(String) }, limit: 100, cursor: null,
  } });
});

it('does not merge distinct log records that share a timestamp and line', async () => {
  const first = { ...logBatch.entries[0], line: 'same line', attributes: { value: 'one' } };
  const second = { ...first, labels: { ...first.labels, service_name: 'other', target: 'other-target' }, attributes: { value: 'two' } };
  const third = { ...first, attributes: { value: 'two' } };
  runHandler = () => Promise.resolve(snapshot('run', { logs: signal({ ...logBatch, entries: [first], next_cursor: 'cursor' }) }));
  logsQueryHandler = args => {
    expect(args.traceId).toBeNull();
    expect(args.request.cursor).toBe('cursor');
    return Promise.resolve(signal({ ...logBatch, entries: [second, third], next_cursor: null }));
  };
  render(<Observability epoch={1} run="run" runStatus="running" />);
  await screen.findByText('run local summary');
  fireEvent.click(screen.getByRole('button', { name: '日志' }));
  fireEvent.click(await screen.findByRole('button', { name: '加载更多' }));
  await waitFor(() => expect(screen.getAllByText('same line')).toHaveLength(3));
  expect(screen.getByText('other-target')).toBeTruthy();
  expect(screen.queryByRole('button', { name: '加载更多' })).toBeNull();
});

it('replaces prior trace pages on refresh and ignores a late same-key page response', async () => {
  let resolvePage!: (value: ObservabilitySignal<ObservabilityTraceList>) => void;
  tracesPageHandler = () => new Promise(resolve => { resolvePage = resolve; });
  let current = snapshot('run', { traces: signal({ ...traceList, next_cursor: 'next-page' }) });
  runHandler = () => Promise.resolve(current);
  render(<Observability epoch={1} run="run" runStatus="running" />);
  fireEvent.click(screen.getByRole('button', { name: 'Traces' }));
  await screen.findByText('root operation');
  fireEvent.click(screen.getByRole('button', { name: '加载更多' }));
  await waitFor(() => expect(api).toHaveBeenCalledWith('observability_traces', expect.anything()));
  current = snapshot('run', { traces: signal({ run_id: 'run', next_cursor: null, traces: [{
    trace_id: 'fresh-trace', root_service_name: 'wukong', root_trace_name: 'fresh first page', start_time: range.end, duration_ms: 5, span_count: 1,
  }] }) });
  fireEvent.click(screen.getByRole('button', { name: '刷新可观察性数据' }));
  expect(await screen.findByText('fresh first page')).toBeTruthy();
  expect(screen.queryByText('root operation')).toBeNull();
  await act(async () => resolvePage(signal({ run_id: 'run', next_cursor: null, traces: [{
    trace_id: 'stale-page', root_service_name: 'wukong', root_trace_name: 'stale late page', start_time: range.start, duration_ms: 5, span_count: 1,
  }] })));
  expect(screen.queryByText('stale late page')).toBeNull();
  expect(screen.getByText('fresh first page')).toBeTruthy();
  expect(screen.queryByRole('button', { name: '加载更多' })).toBeNull();
});

it('opens only the URL returned by the native Grafana command', async () => {
  grafanaHandler = () => Promise.resolve('https://grafana.example/d/native');
  render(<Observability epoch={1} run="run" runStatus="running" />);
  fireEvent.click(screen.getByRole('button', { name: '指标', }));
  fireEvent.click(await screen.findByRole('button', { name: '在 Grafana 中打开' }));
  await waitFor(() => expect(openLink).toHaveBeenCalledWith('https://grafana.example/d/native'));
  expect(vi.mocked(api<'observability_grafana_link'>)).toHaveBeenCalledWith('observability_grafana_link', { epoch: 1, target: { metrics: { metric: 'runs_started' } }, range: { start: expect.any(String), end: expect.any(String) } });
});

it('resets a pending Grafana action when the query key changes', async () => {
  let resolveLink: (value: string | null) => void = () => {};
  grafanaHandler = () => new Promise(resolve => { resolveLink = resolve; });
  render(<Observability epoch={1} run="run" runStatus="running" />);
  fireEvent.click(screen.getByRole('button', { name: '指标' }));
  const button = await screen.findByRole('button', { name: '在 Grafana 中打开' });
  fireEvent.click(button);
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(true));
  fireEvent.change(screen.getByRole('combobox', { name: '指标' }), { target: { value: 'runs_finished' } });
  await waitFor(() => expect((screen.getByRole('button', { name: '在 Grafana 中打开' }) as HTMLButtonElement).disabled).toBe(false));
  await act(async () => { resolveLink(null); await Promise.resolve(); });
  expect(openLink).not.toHaveBeenCalled();
});

it('writes typed settings paths and environment variable names only', async () => {
  const base: Record<string, Json> = { observability: { service_name: 'wukong', otlp_endpoint: 'http://localhost:4318', traces: true, metrics: true,
    logs: false, local_jsonl: true, log_retention_days: 7, shutdown_timeout_secs: 3,
    query: { tempo: { url: 'https://tempo.example', credential_env: 'OLD_NAME' }, loki: null, prometheus: null,
      grafana_url: null, timeout_secs: 5, max_range_secs: 86400 } } };
  let latest: Record<string, Json> = {};
  function Harness() {
    const [edits, setEdits] = useState<Record<string, Json>>({}); latest = edits;
    return <ObservabilitySettings epoch={1} form={editedForm(base, edits)} edits={edits} onEdits={setEdits} disabled={false} />;
  }
  render(<Harness />);
  await screen.findByText('https://tempo.example', { selector: 'span' });
  const tempo = screen.getByText('Tempo', { selector: 'legend' }).closest('fieldset')!;
  const credential = within(tempo).getByLabelText(/^凭据环境变量名/);
  fireEvent.change(credential, { target: { value: 'TEMPO_TOKEN' } });
  expect(latest['observability.query.tempo.credential_env']).toBe('TEMPO_TOKEN');
  expect(JSON.stringify(latest)).not.toContain('secret-value');
  fireEvent.change(credential, { target: { value: '' } });
  expect(latest['observability.query.tempo.credential_env']).toBeNull();
  fireEvent.change(within(tempo).getByLabelText(/后端 URL/), { target: { value: '' } });
  expect(latest['observability.query.tempo']).toBeNull();
  fireEvent.change(screen.getByLabelText('OTLP/HTTP 基础地址'), { target: { value: '' } });
  expect(latest['observability.otlp_endpoint']).toBeNull();
  expect(document.querySelector('.observability-settings input[type="password"]')).toBeNull();
});
