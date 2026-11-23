import { CircleAlert, ExternalLink } from 'lucide-react';
import { useState } from 'react';
import { currentLocale, t } from '../../shared/i18n';
import type { ObservabilityBackendCapability, ObservabilityDeliveryHealth, ObservabilityGrafanaTarget, ObservabilitySignal, ObservabilityStatus, ObservabilityTelemetryStatus } from '../../shared/api/commands';
import type { Json } from '../../shared/api/types';

export function formatTime(value: string | null) {
  if (!value) return t('observability-not-available');
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : new Intl.DateTimeFormat(currentLocale(), { dateStyle: 'short', timeStyle: 'medium' }).format(date);
}
export function formatNumber(value: number) {
  return new Intl.NumberFormat(currentLocale(), { maximumFractionDigits: 4 }).format(value);
}
export function jsonText(value: Json) {
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2);
}

export function BackendStatus({ status, error }: { status?: ObservabilityStatus; error: string }) {
  const entries = status ? ([
    ['Tempo', status.capabilities.tempo], ['Loki', status.capabilities.loki],
    ['Prometheus', status.capabilities.prometheus], ['Grafana', status.capabilities.grafana],
  ] as const) : [];
  return <section className="observability-backends" aria-label={t('observability-backend-status')}>
    {error && <span className="observability-backend-error" role="alert"><CircleAlert size={13} />{error}</span>}
    {!status && !error && <span className="muted">{t('observability-checking-backends')}</span>}
    {entries.map(([name, capability]) => <BackendCapability key={name} name={name} capability={capability} />)}
    {status && <ExporterStatus exporter={status.exporter} />}
  </section>;
}

export function ExporterStatus({ exporter }: { exporter: ObservabilityTelemetryStatus }) {
  const flags = (['activated', 'traces', 'metrics', 'logs', 'local_jsonl', 'worker_relay'] as const);
  const labels: Record<(typeof flags)[number], string> = {
    activated: t('observability-exporter-activated'), traces: t('observability-exporter-traces'),
    metrics: t('observability-exporter-metrics'), logs: t('observability-exporter-logs'),
    local_jsonl: t('observability-exporter-local-jsonl'), worker_relay: t('observability-exporter-worker-relay'),
  };
  const relayKnown = Object.values(exporter.worker_relay_status).some(health => health.healthy !== null || health.last_error !== null);
  return <div className="observability-exporter">
    <strong>{t('observability-exporter-status')}</strong>
    <div className="observability-exporter-flags">{flags.map(flag => <span key={flag} data-enabled={exporter[flag]}>{labels[flag]}: {exporter[flag] ? t('observability-enabled') : t('observability-disabled')}</span>)}</div>
    {(exporter.worker_relay || relayKnown) && <div className="observability-relay-health"><RelayHealth name={t('observability-traces')} health={exporter.worker_relay_status.traces} /><RelayHealth name={t('observability-logs')} health={exporter.worker_relay_status.logs} /></div>}
    {exporter.last_error && <p className="error" role="alert">{t('observability-exporter-last-error')}: {exporter.last_error}</p>}
  </div>;
}
function RelayHealth({ name, health }: { name: string; health: ObservabilityDeliveryHealth }) {
  return <span>{name}: {health.healthy == null ? t('observability-not-available') : health.healthy ? t('observability-healthy') : t('observability-unhealthy')}{health.last_error && <small>{health.last_error}</small>}</span>;
}
function BackendCapability({ name, capability }: { name: string; capability: ObservabilityBackendCapability }) {
  return <details className={`observability-backend ${capability.configured ? 'configured' : ''}`}>
    <summary><span className="observability-presence" aria-hidden="true" />{name}<span>{capability.configured ? t('observability-configured') : t('observability-unconfigured')}</span></summary>
    <div><dl>
      <dt>{t('observability-origin')}</dt><dd>{capability.origin ?? t('observability-not-available')}</dd>
      <dt>{t('observability-credential')}</dt><dd>{capability.credential_available ? t('observability-credential-available') : t('observability-credential-unavailable')}</dd>
    </dl></div>
  </details>;
}

export function SignalNotices({ signal, backend }: { signal?: ObservabilitySignal<unknown>; backend: 'tempo' | 'loki' | 'prometheus' }) {
  if (!signal) return null;
  return <div className="observability-notices" aria-live="polite">
    {!signal.configured && <p className="observability-unconfigured-message">{t('observability-backend-not-configured', { backend })}</p>}
    {signal.stale && <span className="observability-flag stale">{t('observability-stale')}</span>}
    {signal.partial && <span className="observability-flag partial">{t('observability-partial')}</span>}
    {signal.truncated && <span className="observability-flag truncated">{t('observability-truncated')}</span>}
    {signal.error && <p className="error" role="alert"><strong>{signal.error.kind}</strong> {signal.error.message}{signal.error.status != null && ` (${signal.error.status})`}</p>}
  </div>;
}

export function LocalNotices({ partial, truncated, stale }: { partial: boolean; truncated: boolean; stale: boolean }) {
  return <div className="observability-notices" aria-live="polite">
    {stale && <span className="observability-flag stale">{t('observability-stale')}</span>}
    {partial && <span className="observability-flag partial">{t('observability-partial')}</span>}
    {truncated && <span className="observability-flag truncated">{t('observability-truncated')}</span>}
  </div>;
}

export function GrafanaButton({ target, onOpen, disabled = false }: {
  target: ObservabilityGrafanaTarget; onOpen: (target: ObservabilityGrafanaTarget) => Promise<void>; disabled?: boolean;
}) {
  const [pending, setPending] = useState(false);
  return <button className="subtle" disabled={disabled || pending} onClick={() => {
    setPending(true); void onOpen(target).finally(() => setPending(false));
  }}><ExternalLink size={14} />{t('observability-open-grafana')}</button>;
}
