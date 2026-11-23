import { useCallback, useEffect, useRef, useState } from 'react';
import { api, errorText, openLink } from '../../shared/api/bridge';
import type {
  ObservabilityGrafanaTarget, ObservabilityLocalActivity, ObservabilityLogBatch, ObservabilityLogEntry,
  ObservabilityMetricName, ObservabilityRunSnapshot, ObservabilitySignal, ObservabilityStatus,
  ObservabilityTimeRange, ObservabilityTraceList, ObservabilityTraceSummary,
} from '../../shared/api/commands';
import type { Json } from '../../shared/api/types';

export const OBSERVABILITY_METRICS: ObservabilityMetricName[] = [
  'runs_started', 'runs_finished', 'active_runs', 'run_duration_count', 'agent_generation_duration_count',
  'worker_rpc_duration_count', 'worker_rpc_retries', 'model_operation_duration_count', 'model_token_usage_count',
  'evidence_sync_duration_count', 'telemetry_dropped',
];
export const OBSERVABILITY_PAGE_LIMIT = 100;

export function observabilityTimeRange(minutes: number, now = Date.now()): ObservabilityTimeRange {
  return { start: new Date(now - minutes * 60_000).toISOString(), end: new Date(now).toISOString() };
}

/** Native failures intentionally carry data=null. Keep only data from the same request generation. */
export function retainSignal<T>(next: ObservabilitySignal<T>, prior?: ObservabilitySignal<T>): ObservabilitySignal<T> {
  if (next.data === null && prior?.data != null && (next.error || next.stale)) return { ...next, data: prior.data, stale: true };
  return next;
}

function appendLocal(next: ObservabilityLocalActivity, current: ObservabilityLocalActivity): ObservabilityLocalActivity {
  const events = new Map(current.events.map(event => [event.seq, event]));
  next.events.forEach(event => events.set(event.seq, event));
  return { ...next, events: [...events.values()].sort((a, b) => a.seq - b.seq) };
}

function compareTraces(left: ObservabilityTraceSummary, right: ObservabilityTraceSummary) {
  const leftTime = left.start_time ? Date.parse(left.start_time) : Number.NEGATIVE_INFINITY;
  const rightTime = right.start_time ? Date.parse(right.start_time) : Number.NEGATIVE_INFINITY;
  return rightTime - leftTime || left.trace_id.localeCompare(right.trace_id);
}
function appendTraces(next: ObservabilitySignal<ObservabilityTraceList>, current: ObservabilitySignal<ObservabilityTraceList>) {
  const retained = retainSignal(next, current);
  if (!retained.data || next.data == null) return retained;
  const traces = new Map(current.data?.traces.map(trace => [trace.trace_id, trace]) ?? []);
  retained.data.traces.forEach(trace => traces.set(trace.trace_id, trace));
  return { ...retained, data: { ...retained.data, traces: [...traces.values()].sort(compareTraces) } };
}

/** Stable across JSON object key order, and includes every field that distinguishes log records. */
export function stableObservabilityJson(value: Json): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value) ?? 'null';
  if (Array.isArray(value)) return `[${value.map(stableObservabilityJson).join(',')}]`;
  return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${stableObservabilityJson(value[key] ?? null)}`).join(',')}}`;
}
export function observabilityLogIdentity(entry: ObservabilityLogEntry): string {
  return stableObservabilityJson({
    timestamp: entry.timestamp, timestamp_nanos: entry.timestamp_nanos, line: entry.line, line_truncated: entry.line_truncated,
    labels: { service_name: entry.labels.service_name, level: entry.labels.level, target: entry.labels.target },
    attributes: entry.attributes,
  });
}
function compareLogs(left: ObservabilityLogEntry, right: ObservabilityLogEntry) {
  const time = Date.parse(right.timestamp) - Date.parse(left.timestamp);
  if (time) return time;
  if (left.timestamp_nanos !== right.timestamp_nanos) {
    return right.timestamp_nanos.length - left.timestamp_nanos.length || right.timestamp_nanos.localeCompare(left.timestamp_nanos);
  }
  return observabilityLogIdentity(left).localeCompare(observabilityLogIdentity(right));
}
function appendLogs(next: ObservabilitySignal<ObservabilityLogBatch>, current: ObservabilitySignal<ObservabilityLogBatch>) {
  const retained = retainSignal(next, current);
  if (!retained.data || next.data == null) return retained;
  const entries = new Map(current.data?.entries.map(entry => [observabilityLogIdentity(entry), entry]) ?? []);
  retained.data.entries.forEach(entry => entries.set(observabilityLogIdentity(entry), entry));
  return { ...retained, data: { ...retained.data, entries: [...entries.values()].sort(compareLogs) } };
}

/** A refreshed first page replaces prior pages so sliding ranges cannot retain aged-out records. */
function replaceSnapshot(next: ObservabilityRunSnapshot, prior?: ObservabilityRunSnapshot): ObservabilityRunSnapshot {
  return {
    local: next.local,
    traces: retainSignal(next.traces, prior?.traces),
    logs: retainSignal(next.logs, prior?.logs),
    metrics: retainSignal(next.metrics, prior?.metrics),
  };
}

type SnapshotState = { key: string; value?: ObservabilityRunSnapshot };
type ErrorState = { key: string; value: string };
type PageToken = { key: string; generation: number; sequence: number };

export function useObservability({ epoch, run, runStatus, rangeMinutes, metric, logTrace }: {
  epoch: number; run: string; runStatus: string; rangeMinutes: number; metric: ObservabilityMetricName; logTrace: string;
}) {
  const requestKey = `${epoch}:${run}:${rangeMinutes}:${metric}:${logTrace}`;
  const keyRef = useRef(requestKey);
  keyRef.current = requestKey;
  const generationRef = useRef(0);
  const refreshActiveRef = useRef(false);
  const pageSequenceRef = useRef(0);
  const pageActiveRef = useRef<number | null>(null);
  const [snapshot, setSnapshot] = useState<SnapshotState>({ key: requestKey });
  const [status, setStatus] = useState<{ epoch: number; value?: ObservabilityStatus }>({ epoch });
  const [error, setError] = useState<ErrorState>({ key: requestKey, value: '' });
  const [statusError, setStatusError] = useState<{ epoch: number; value: string }>({ epoch, value: '' });
  const [loading, setLoading] = useState<{ key: string; value: boolean }>({ key: requestKey, value: true });
  const [pageBusy, setPageBusy] = useState<'local' | 'traces' | 'logs' | null>(null);
  const [pageError, setPageError] = useState<ErrorState>({ key: requestKey, value: '' });
  const [linkBusy, setLinkBusy] = useState(false);
  const [linkMessage, setLinkMessage] = useState<ErrorState>({ key: requestKey, value: '' });
  const reloadRef = useRef<() => void>(() => {});
  const data = snapshot.key === requestKey ? snapshot.value : undefined;
  const dataRef = useRef(data);
  dataRef.current = data;

  useEffect(() => {
    let disposed = false, inFlight = false, again = false;
    let timer: number | undefined;
    setPageBusy(null);
    setLinkBusy(false);
    const schedule = () => {
      if (!disposed) timer = window.setTimeout(() => { void load(); }, ['running', 'waiting', 'queued'].includes(runStatus) ? 5000 : 15000);
    };
    const load = async () => {
      if (disposed) return;
      if (inFlight) { again = true; return; }
      if (timer !== undefined) window.clearTimeout(timer);
      if (document.visibilityState === 'hidden') { schedule(); return; }
      inFlight = true;
      const generation = ++generationRef.current;
      refreshActiveRef.current = true;
      pageSequenceRef.current += 1;
      pageActiveRef.current = null;
      setPageBusy(null);
      setLoading({ key: requestKey, value: true });
      const range = observabilityTimeRange(rangeMinutes);
      const request = { run, range, limit: OBSERVABILITY_PAGE_LIMIT, trace_cursor: null, log_cursor: null, local_cursor: null, metric };
      const filteredLogs = logTrace ? api('observability_logs', { epoch, run, traceId: logTrace,
        request: { range, limit: OBSERVABILITY_PAGE_LIMIT, cursor: null } }) : Promise.resolve(null);
      const [next, capabilities, filtered] = await Promise.allSettled([
        api('observability_run', { epoch, request }), api('observability_status', { epoch }), filteredLogs,
      ]);
      const valid = () => !disposed && keyRef.current === requestKey && generationRef.current === generation;
      if (!valid()) return;
      if (next.status === 'fulfilled') {
        let value = next.value;
        if (logTrace) {
          if (filtered.status === 'fulfilled' && filtered.value) value = { ...value, logs: filtered.value };
          else if (filtered.status === 'rejected') value = { ...value, logs: { ...value.logs, data: null, partial: true,
            error: { kind: 'request', message: errorText(filtered.reason) } } };
        }
        setSnapshot(current => valid() ? {
          key: requestKey, value: replaceSnapshot(value, current.key === requestKey ? current.value : undefined),
        } : current);
        setError({ key: requestKey, value: '' });
      } else {
        setSnapshot(current => {
          if (!valid()) return current;
          const prior = current.key === requestKey ? current.value : undefined;
          if (!prior) return current.key === requestKey ? current : { key: requestKey };
          return { key: requestKey, value: { ...prior, local: { ...prior.local, stale: true },
            traces: { ...prior.traces, stale: true }, logs: { ...prior.logs, stale: true }, metrics: { ...prior.metrics, stale: true } } };
        });
        setError({ key: requestKey, value: errorText(next.reason) });
      }
      if (capabilities.status === 'fulfilled') {
        setStatus({ epoch, value: capabilities.value });
        setStatusError({ epoch, value: '' });
      } else setStatusError({ epoch, value: errorText(capabilities.reason) });
      setLoading({ key: requestKey, value: false });
      refreshActiveRef.current = false;
      inFlight = false;
      if (again) { again = false; void load(); } else schedule();
    };
    reloadRef.current = () => { void load(); };
    const catchUp = () => { void load(); };
    const visible = () => { if (document.visibilityState === 'visible') catchUp(); };
    window.addEventListener('focus', catchUp);
    document.addEventListener('visibilitychange', visible);
    void load();
    return () => {
      disposed = true;
      refreshActiveRef.current = false;
      generationRef.current += 1;
      pageSequenceRef.current += 1;
      pageActiveRef.current = null;
      if (timer !== undefined) window.clearTimeout(timer);
      window.removeEventListener('focus', catchUp);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [epoch, run, runStatus, rangeMinutes, metric, logTrace, requestKey]);

  const beginPage = (kind: 'local' | 'traces' | 'logs'): PageToken | null => {
    if (refreshActiveRef.current || pageActiveRef.current !== null) return null;
    const token = { key: keyRef.current, generation: generationRef.current, sequence: ++pageSequenceRef.current };
    pageActiveRef.current = token.sequence;
    setPageBusy(kind); setPageError({ key: token.key, value: '' });
    return token;
  };
  const validPage = (token: PageToken) => keyRef.current === token.key && generationRef.current === token.generation && pageSequenceRef.current === token.sequence;
  const finishPage = (token: PageToken) => {
    if (pageSequenceRef.current !== token.sequence) return;
    pageActiveRef.current = null;
    if (validPage(token)) setPageBusy(null);
  };
  const appendPage = <K extends 'local' | 'traces' | 'logs'>(token: PageToken, field: K, append: (current: ObservabilityRunSnapshot) => ObservabilityRunSnapshot[K]) => {
    setSnapshot(state => validPage(token) && state.key === token.key && state.value
      ? { key: token.key, value: { ...state.value, [field]: append(state.value) } } : state);
  };

  const loadMoreLocal = useCallback(async () => {
    const current = dataRef.current;
    if (!current?.local.next_cursor) return;
    const token = beginPage('local');
    if (!token) return;
    try {
      const next = await api('observability_local_activity', { epoch, run, cursor: current.local.next_cursor, limit: OBSERVABILITY_PAGE_LIMIT });
      if (validPage(token)) appendPage(token, 'local', value => appendLocal(next, value.local));
    } catch (cause) { if (validPage(token)) setPageError({ key: token.key, value: errorText(cause) }); }
    finally { finishPage(token); }
  }, [epoch, run]);

  const loadMoreTraces = useCallback(async () => {
    const current = dataRef.current;
    if (!current?.traces.data?.next_cursor) return;
    const token = beginPage('traces');
    if (!token) return;
    try {
      const next = await api('observability_traces', { epoch, run, request: {
        range: observabilityTimeRange(rangeMinutes), limit: OBSERVABILITY_PAGE_LIMIT, cursor: current.traces.data.next_cursor,
      } });
      if (validPage(token)) appendPage(token, 'traces', value => appendTraces(next, value.traces));
    } catch (cause) { if (validPage(token)) setPageError({ key: token.key, value: errorText(cause) }); }
    finally { finishPage(token); }
  }, [epoch, run, rangeMinutes]);

  const loadMoreLogs = useCallback(async () => {
    const current = dataRef.current;
    if (!current?.logs.data?.next_cursor) return;
    const token = beginPage('logs');
    if (!token) return;
    try {
      const next = await api('observability_logs', { epoch, run, traceId: logTrace || null, request: {
        range: observabilityTimeRange(rangeMinutes), limit: OBSERVABILITY_PAGE_LIMIT, cursor: current.logs.data.next_cursor,
      } });
      if (validPage(token)) appendPage(token, 'logs', value => appendLogs(next, value.logs));
    } catch (cause) { if (validPage(token)) setPageError({ key: token.key, value: errorText(cause) }); }
    finally { finishPage(token); }
  }, [epoch, run, rangeMinutes, logTrace]);

  const openGrafana = useCallback(async (target: ObservabilityGrafanaTarget) => {
    const key = keyRef.current;
    setLinkBusy(true); setLinkMessage({ key, value: '' });
    try {
      const url = await api('observability_grafana_link', { epoch, target, range: observabilityTimeRange(rangeMinutes) });
      if (keyRef.current !== key) return;
      if (!url) setLinkMessage({ key, value: 'unconfigured' });
      else await openLink(url);
    } catch (cause) { if (keyRef.current === key) setLinkMessage({ key, value: errorText(cause) }); }
    finally { if (keyRef.current === key) setLinkBusy(false); }
  }, [epoch, rangeMinutes]);

  return {
    data, status: status.epoch === epoch ? status.value : undefined,
    error: error.key === requestKey ? error.value : '',
    statusError: statusError.epoch === epoch ? statusError.value : '',
    loading: loading.key === requestKey ? loading.value : true,
    pageBusy, pageError: pageError.key === requestKey ? pageError.value : '',
    linkBusy, linkMessage: linkMessage.key === requestKey ? linkMessage.value : '',
    reload: useCallback(() => reloadRef.current(), []), loadMoreLocal, loadMoreTraces, loadMoreLogs, openGrafana,
  };
}
