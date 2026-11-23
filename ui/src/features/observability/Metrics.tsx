import { useEffect, useMemo, useRef, useState } from 'react';
import * as echarts from 'echarts';
import { t } from '../../shared/i18n';
import type {
  ObservabilityGrafanaTarget, ObservabilityMetricBatch, ObservabilityMetricName, ObservabilitySignal,
} from '../../shared/api/commands';
import { OBSERVABILITY_METRICS } from './model';
import { formatNumber, formatTime, GrafanaButton, SignalNotices } from './Status';

export function metricLabel(metric: ObservabilityMetricName) {
  const labels: Record<ObservabilityMetricName, string> = {
    runs_started: t('observability-metric-runs-started'), runs_finished: t('observability-metric-runs-finished'),
    active_runs: t('observability-metric-active-runs'), run_duration_count: t('observability-metric-run-duration-count'),
    agent_generation_duration_count: t('observability-metric-agent-generation-duration-count'),
    worker_rpc_duration_count: t('observability-metric-worker-rpc-duration-count'), worker_rpc_retries: t('observability-metric-worker-rpc-retries'),
    model_operation_duration_count: t('observability-metric-model-operation-duration-count'),
    model_token_usage_count: t('observability-metric-model-token-usage-count'),
    evidence_sync_duration_count: t('observability-metric-evidence-sync-duration-count'), telemetry_dropped: t('observability-metric-telemetry-dropped'),
  };
  return labels[metric];
}
function seriesName(labels: Record<string, string>, metric: ObservabilityMetricName) {
  const value = Object.entries(labels).map(([key, item]) => `${key}=${item}`).join(', ');
  return value || metricLabel(metric);
}
function Chart({ batch }: { batch: ObservabilityMetricBatch }) {
  const element = useRef<HTMLDivElement>(null);
  const series = useMemo(() => batch.series.slice(0, 8), [batch.series]);
  useEffect(() => {
    if (!element.current) return;
    const host = element.current;
    const styles = getComputedStyle(host);
    const token = (name: string) => styles.getPropertyValue(name).trim();
    const colors = Array.from({ length: 8 }, (_, index) => token(`--observability-series-${index + 1}`));
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const chart = echarts.init(host, undefined, { renderer: 'canvas' });
    chart.setOption({
      animation: !reduced, color: colors,
      aria: { enabled: true, decal: { show: false }, description: `${metricLabel(batch.metric)} ${t('observability-chart-description')}` },
      grid: { left: 58, right: series.length <= 4 ? 190 : 24, top: series.length > 1 ? 48 : 24, bottom: 48, containLabel: true },
      tooltip: { trigger: 'axis', renderMode: 'richText', axisPointer: { type: 'cross', label: { show: true } }, valueFormatter: (value: unknown) => formatNumber(Number(value)) },
      legend: series.length > 1 ? { type: 'scroll', top: 0, textStyle: { color: token('--muted') } } : undefined,
      xAxis: { type: 'time', axisLine: { lineStyle: { color: token('--border-strong') } }, axisLabel: { color: token('--muted') }, splitLine: { show: false } },
      yAxis: { type: 'value', minInterval: batch.metric.endsWith('_count') || ['runs_started', 'runs_finished', 'active_runs', 'worker_rpc_retries', 'telemetry_dropped'].includes(batch.metric) ? 1 : undefined,
        axisLabel: { color: token('--muted') }, splitLine: { lineStyle: { color: token('--border'), width: 1, type: 'solid' } } },
      series: series.map((item, index) => ({
        id: seriesName(item.labels, batch.metric), name: seriesName(item.labels, batch.metric), type: 'line' as const,
        data: item.points.map(point => [point.timestamp, point.value]), showSymbol: false, symbolSize: 8,
        lineStyle: { width: 2 }, itemStyle: { color: colors[index] }, emphasis: { focus: 'series' as const },
        endLabel: { show: series.length <= 4, formatter: seriesName(item.labels, batch.metric), color: token('--text'), fontSize: 12 },
      })),
    }, { notMerge: true });
    const resize = () => chart.resize();
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(resize);
    observer?.observe(host); window.addEventListener('resize', resize);
    return () => { observer?.disconnect(); window.removeEventListener('resize', resize); chart.dispose(); };
  }, [batch, series]);
  return <div ref={element} className="observability-chart" role="img" tabIndex={0} aria-label={`${metricLabel(batch.metric)} ${t('observability-line-chart')}`} />;
}

export function Metrics({ signal, metric, onMetric, onGrafana, grafanaDisabled }: {
  signal?: ObservabilitySignal<ObservabilityMetricBatch>; metric: ObservabilityMetricName;
  onMetric: (metric: ObservabilityMetricName) => void; onGrafana: (target: ObservabilityGrafanaTarget) => Promise<void>; grafanaDisabled: boolean;
}) {
  const [table, setTable] = useState(false);
  const batch = signal?.data;
  return <section className="observability-signal-view" aria-labelledby="observability-metrics-heading">
    <div className="observability-view-heading"><div><h3 id="observability-metrics-heading">{t('observability-metrics')}</h3><p className="muted">{t('observability-metrics-description')}</p></div>
      <GrafanaButton target={{ metrics: { metric } }} onOpen={onGrafana} disabled={grafanaDisabled} /></div>
    <div className="observability-filter"><label><span>{t('observability-metric')}</span><select value={metric} onChange={event => onMetric(event.target.value as ObservabilityMetricName)}>
      {OBSERVABILITY_METRICS.map(value => <option key={value} value={value}>{metricLabel(value)}</option>)}
    </select></label><button aria-pressed={table} onClick={() => setTable(value => !value)}>{table ? t('observability-show-chart') : t('observability-show-table')}</button></div>
    <SignalNotices signal={signal} backend="prometheus" />
    {batch && <>
      <div className="observability-query-meta"><span>{t('observability-service-scope')}</span><code>{batch.expression}</code><span>{t('observability-step-seconds', { seconds: batch.step_seconds })}</span></div>
      {batch.series.length > 8 && <p className="observability-flag truncated">{t('observability-chart-series-limited', { shown: 8, total: batch.series.length })}</p>}
      {table ? <MetricTable batch={batch} /> : batch.series.length ? <Chart batch={batch} /> : <p className="muted">{t('observability-no-metric-points')}</p>}
      {!table && batch.series.length > 0 && <details className="observability-data-table"><summary>{t('observability-view-data-table')}</summary><MetricTable batch={batch} /></details>}
    </>}
  </section>;
}
function MetricTable({ batch }: { batch: ObservabilityMetricBatch }) {
  return <div className="observability-table-scroll"><table className="observability-table"><thead><tr><th>{t('observability-series')}</th><th>{t('observability-timestamp')}</th><th>{t('observability-value')}</th></tr></thead>
    <tbody>{batch.series.flatMap(item => item.points.map((point, index) => <tr key={`${seriesName(item.labels, batch.metric)}:${point.timestamp}:${index}`}><td>{seriesName(item.labels, batch.metric)}</td><td>{formatTime(point.timestamp)}</td><td>{formatNumber(point.value)}</td></tr>))}</tbody></table></div>;
}
