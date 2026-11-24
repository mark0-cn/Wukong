import { useEffect, useMemo, useRef, useState } from 'react';
import { DataSet } from 'vis-data/peer';
import { Timeline, type DataItemCollectionType, type TimelineGroup, type TimelineItem } from 'vis-timeline/peer';
import 'vis-timeline/styles/vis-timeline-graph2d.min.css';
import { t } from '../../shared/i18n';
import type {
  ObservabilityGrafanaTarget, ObservabilitySignal, ObservabilityTrace, ObservabilityTraceList, ObservabilityTraceSpan,
} from '../../shared/api/commands';
import { api, errorText } from '../../shared/api/bridge';
import { formatNumber, formatTime, GrafanaButton, jsonText, SignalNotices } from './Status';

function orderedSpans(spans: ObservabilityTraceSpan[]) {
  const byId = new Map(spans.map(span => [span.span_id, span]));
  const children = new Map<string | null, ObservabilityTraceSpan[]>();
  spans.forEach(span => {
    const parent = span.parent_span_id && byId.has(span.parent_span_id) ? span.parent_span_id : null;
    children.set(parent, [...(children.get(parent) ?? []), span]);
  });
  const result: { span: ObservabilityTraceSpan; depth: number }[] = [];
  const visited = new Set<string>();
  const visit = (span: ObservabilityTraceSpan, depth: number) => {
    if (visited.has(span.span_id)) return;
    visited.add(span.span_id); result.push({ span, depth });
    children.get(span.span_id)?.forEach(child => visit(child, depth + 1));
  };
  children.get(null)?.forEach(span => visit(span, 0));
  spans.forEach(span => visit(span, 0));
  return result;
}
function textElement(text: string, className: string, depth = 0) {
  const element = document.createElement('span');
  element.className = className; element.textContent = text; element.style.paddingLeft = `${depth * 14}px`;
  return element;
}
function SpanTimeline({ trace, selected, onSelect }: { trace: ObservabilityTrace; selected: string; onSelect: (id: string) => void }) {
  const container = useRef<HTMLDivElement>(null);
  const timelineRef = useRef<Timeline | null>(null);
  const ordered = useMemo(() => orderedSpans(trace.spans), [trace.spans]);
  useEffect(() => {
    if (!container.current) return;
    const groups = new DataSet<TimelineGroup>(ordered.map(({ span, depth }) => ({
      id: span.span_id, content: textElement(span.name, 'observability-timeline-group', depth),
    })));
    const items = new DataSet<TimelineItem>(ordered.flatMap(({ span }) => {
      const start = span.start_time ?? span.end_time;
      if (!start) return [];
      const end = span.start_time && span.end_time && Date.parse(span.end_time) >= Date.parse(span.start_time) ? span.end_time : undefined;
      return [{ id: span.span_id, group: span.span_id, start, end, type: end ? 'range' as const : 'box' as const,
        content: textElement(span.name, 'observability-timeline-item'), className: span.status === 'error' ? 'span-error' : 'span-default' }];
    }));
    const timeline = new Timeline(container.current, items as unknown as DataItemCollectionType, groups, {
      height: '320px', autoResize: true, selectable: true, multiselect: false, stack: false,
      verticalScroll: true, horizontalScroll: true, zoomKey: 'ctrlKey', showCurrentTime: false,
      orientation: { axis: 'both', item: 'top' }, margin: { item: { horizontal: 4, vertical: 6 }, axis: 8 },
    });
    timeline.on('select', (properties: { items?: (string | number)[] }) => {
      const id = properties.items?.[0]; if (id != null) onSelect(String(id));
    });
    timelineRef.current = timeline;
    const timestamps = ordered.flatMap(({ span }) => [span.start_time, span.end_time]
      .map(value => value ? Date.parse(value) : Number.NaN).filter(Number.isFinite));
    if (timestamps.length) {
      const start = Math.min(...timestamps);
      const end = Math.max(...timestamps);
      const padding = Math.max((end - start) * 0.08, 10);
      timeline.setWindow(new Date(start - padding), new Date(end + padding), { animation: false });
    }
    return () => { timelineRef.current = null; timeline.destroy(); };
  }, [ordered, onSelect]);
  useEffect(() => { timelineRef.current?.setSelection(selected ? [selected] : []); }, [selected, ordered]);
  return <div className="observability-timeline" ref={container} aria-label={t('observability-span-timeline')} />;
}

function SpanProperties({ span }: { span: ObservabilityTraceSpan }) {
  return <section className="observability-span-details" aria-label={t('observability-selected-span')}>
    <div className="observability-section-heading"><h4>{span.name}</h4><code>{span.span_id}</code></div>
    <dl className="observability-span-facts"><div><dt>{t('observability-start')}</dt><dd>{formatTime(span.start_time)}</dd></div>
      <div><dt>{t('observability-duration')}</dt><dd>{span.duration_nanos == null ? t('observability-not-available') : `${formatNumber(span.duration_nanos / 1_000_000)} ms`}</dd></div>
      <div><dt>{t('observability-state')}</dt><dd>{span.status ?? t('observability-not-available')}</dd></div>
      <div><dt>{t('observability-parent-span')}</dt><dd>{span.parent_span_id ?? t('observability-root-span')}</dd></div></dl>
    <div className="observability-property-columns"><PropertyList title={t('observability-attributes')} values={span.attributes} /><PropertyList title={t('observability-resource')} values={span.resource} /></div>
  </section>;
}
function PropertyList({ title, values }: { title: string; values: ObservabilityTraceSpan['attributes'] }) {
  return <div><h4>{title}</h4>{Object.keys(values).length ? <dl className="observability-properties">{Object.entries(values).map(([key, value]) =>
    <div key={key}><dt>{key}</dt><dd><pre>{jsonText(value)}</pre></dd></div>)}</dl> : <p className="muted">{t('observability-no-properties')}</p>}</div>;
}

export function Traces({ signal, run, epoch, onLoadMore, loadingMore, onGrafana, grafanaDisabled }: {
  signal?: ObservabilitySignal<ObservabilityTraceList>; run: string; epoch: number; onLoadMore: () => void; loadingMore: boolean;
  onGrafana: (target: ObservabilityGrafanaTarget) => Promise<void>; grafanaDisabled: boolean;
}) {
  const [selectedTrace, setSelectedTrace] = useState('');
  const [selectedSpan, setSelectedSpan] = useState('');
  const [detail, setDetail] = useState<{ key: string; signal?: ObservabilitySignal<ObservabilityTrace>; error: string }>({ key: '', error: '' });
  const detailKey = `${epoch}:${run}:${selectedTrace}`;
  useEffect(() => {
    if (!selectedTrace) return;
    let alive = true;
    api('observability_trace', { epoch, run, traceId: selectedTrace }).then(next => {
      if (!alive) return;
      setDetail(current => {
        const prior = current.key === detailKey ? current.signal : undefined;
        const retained = next.data === null && prior?.data != null && (next.error || next.stale) ? { ...next, data: prior.data, stale: true } : next;
        return { key: detailKey, signal: retained, error: '' };
      });
    }).catch(cause => { if (alive) setDetail(current => ({ key: detailKey, signal: current.key === detailKey ? current.signal : undefined, error: errorText(cause) })); });
    return () => { alive = false; };
  }, [epoch, run, selectedTrace, detailKey]);
  const currentDetail = detail.key === detailKey ? detail : undefined;
  const trace = currentDetail?.signal?.data;
  const span = trace?.spans.find(item => item.span_id === selectedSpan) ?? trace?.spans[0];
  const traces = signal?.data?.traces ?? [];
  return <section className="observability-signal-view" aria-labelledby="observability-traces-heading">
    <div className="observability-view-heading"><div><h3 id="observability-traces-heading">{t('observability-traces')}</h3><p className="muted">{t('observability-traces-description')}</p></div>
      <GrafanaButton target={{ traces: { run } }} onOpen={onGrafana} disabled={grafanaDisabled} /></div>
    <SignalNotices signal={signal} backend="tempo" />
    {signal?.data && <div className="observability-trace-layout">
      <div className="observability-trace-list" role="list" aria-label={t('observability-traces')}>
        {traces.map(item => <div role="listitem" key={item.trace_id}><button className={selectedTrace === item.trace_id ? 'selected' : ''} onClick={() => { setSelectedTrace(item.trace_id); setSelectedSpan(''); }}>
          <strong>{item.root_trace_name ?? item.trace_id}</strong><code>{item.trace_id}</code>
          <span>{item.root_service_name ?? t('observability-unknown-service')} · {formatNumber(item.duration_ms)} ms</span>
          <small>{formatTime(item.start_time)} · {t('observability-span-count', { count: item.span_count })}</small>
        </button></div>)}
        {!traces.length && <p className="muted">{t('observability-no-traces')}</p>}
        {signal.data.next_cursor && <button disabled={loadingMore} onClick={onLoadMore}>{loadingMore ? t('observability-loading') : t('observability-load-more')}</button>}
      </div>
      <div className="observability-trace-detail">
        {!selectedTrace && <p className="muted">{t('observability-select-trace')}</p>}
        {selectedTrace && currentDetail?.error && <p className="error" role="alert">{currentDetail.error}</p>}
        {currentDetail?.signal && <SignalNotices signal={currentDetail.signal} backend="tempo" />}
        {trace && <>
          <div className="observability-trace-detail-heading"><div><h4>{t('observability-trace-detail')}</h4><code>{trace.trace_id}</code></div>
            <label><span>{t('observability-select-span')}</span><select value={span?.span_id ?? ''} onChange={event => setSelectedSpan(event.target.value)}>
              {trace.spans.map(item => <option key={item.span_id} value={item.span_id}>{item.name} ({item.span_id})</option>)}
            </select></label></div>
          {trace.spans.length ? <SpanTimeline trace={trace} selected={span?.span_id ?? ''} onSelect={setSelectedSpan} /> : <p className="muted">{t('observability-no-spans')}</p>}
          {span && <SpanProperties span={span} />}
        </>}
      </div>
    </div>}
  </section>;
}
