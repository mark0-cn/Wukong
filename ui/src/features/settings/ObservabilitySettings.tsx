import { useEffect, useState, type Dispatch, type SetStateAction } from 'react';
import { t } from '../../shared/i18n';
import { api, errorText } from '../../shared/api/bridge';
import type { ObservabilityBackendCapability, ObservabilityStatus } from '../../shared/api/commands';
import type { Json } from '../../shared/api/types';
import { atPath } from './transport';
import { ExporterStatus } from '../observability/Status';

type Form = Record<string, Json>;
type Edits = Record<string, Json>;
type EditState = Dispatch<SetStateAction<Edits>>;
type BackendName = 'tempo' | 'loki' | 'prometheus';

export function ObservabilitySettings({ epoch, form, onEdits, disabled }: {
  epoch: number; form: Form; edits: Edits; onEdits: EditState; disabled: boolean;
}) {
  const [status, setStatus] = useState<{ epoch: number; value?: ObservabilityStatus; error: string }>({ epoch, error: '' });
  useEffect(() => {
    let alive = true;
    api('observability_status', { epoch }).then(value => { if (alive) setStatus({ epoch, value, error: '' }); })
      .catch(cause => { if (alive) setStatus({ epoch, error: errorText(cause) }); });
    return () => { alive = false; };
  }, [epoch]);
  const change = (path: string, value: Json) => onEdits(current => ({ ...current, [path]: value }));
  const currentStatus = status.epoch === epoch ? status : undefined;
  return <div className="observability-settings">
    <section className="settings-group"><h3>{t('observability-settings-export')}</h3><p className="muted">{t('observability-settings-export-help')}</p>
      <div className="settings-grid">
        <TextField label={t('observability-settings-service-name')} value={atPath(form, 'observability.service_name')} disabled={disabled} onChange={value => change('observability.service_name', value ?? '')} />
        <TextField label={t('observability-settings-otlp-endpoint')} value={atPath(form, 'observability.otlp_endpoint')} disabled={disabled} nullable onChange={value => change('observability.otlp_endpoint', value)} />
        <NumberField label={t('observability-settings-retention')} value={atPath(form, 'observability.log_retention_days')} min={1} max={90} disabled={disabled} onChange={value => change('observability.log_retention_days', value)} />
        <NumberField label={t('observability-settings-shutdown-timeout')} value={atPath(form, 'observability.shutdown_timeout_secs')} min={1} max={30} disabled={disabled} onChange={value => change('observability.shutdown_timeout_secs', value)} />
        {(['traces', 'metrics', 'logs', 'local_jsonl'] as const).map(value => {
          const labels = { traces: t('observability-settings-traces'), metrics: t('observability-settings-metrics'), logs: t('observability-settings-logs'), local_jsonl: t('observability-settings-local-jsonl') };
          return <CheckboxField key={value} label={labels[value]} checked={Boolean(atPath(form, `observability.${value}`))} disabled={disabled} onChange={checked => change(`observability.${value}`, checked)} />;
        })}
      </div>
    </section>
    <section className="settings-group"><h3>{t('observability-settings-query')}</h3><p className="muted">{t('observability-settings-query-help')}</p>
      <div className="settings-grid">
        {(['tempo', 'loki', 'prometheus'] as const).map(name => <BackendFields key={name} name={name} form={form} disabled={disabled} onEdits={onEdits} />)}
        <TextField label={t('observability-settings-grafana-url')} value={atPath(form, 'observability.query.grafana_url')} disabled={disabled} nullable onChange={value => change('observability.query.grafana_url', value)} />
        <NumberField label={t('observability-settings-query-timeout')} value={atPath(form, 'observability.query.timeout_secs')} min={1} max={60} disabled={disabled} onChange={value => change('observability.query.timeout_secs', value)} />
        <NumberField label={t('observability-settings-max-range')} value={atPath(form, 'observability.query.max_range_secs')} min={60} max={2678400} disabled={disabled} onChange={value => change('observability.query.max_range_secs', value)} />
      </div>
    </section>
    <section className="settings-group"><h3>{t('observability-backend-status')}</h3>
      {currentStatus?.error && <p className="error" role="alert">{currentStatus.error}</p>}
      {!currentStatus?.value && !currentStatus?.error && <p className="muted">{t('observability-checking-backends')}</p>}
      {currentStatus?.value && <><div className="observability-settings-status">{(['tempo', 'loki', 'prometheus', 'grafana'] as const).map(name =>
        <StatusRow key={name} name={name} capability={currentStatus.value!.capabilities[name]} />)}</div><ExporterStatus exporter={currentStatus.value.exporter} /></>}
    </section>
  </div>;
}
function StatusRow({ name, capability }: { name: string; capability: ObservabilityBackendCapability }) {
  return <div><strong>{name === 'tempo' ? 'Tempo' : name === 'loki' ? 'Loki' : name === 'prometheus' ? 'Prometheus' : 'Grafana'}</strong>
    <span>{capability.configured ? t('observability-configured') : t('observability-unconfigured')}</span><span>{capability.origin ?? t('observability-not-available')}</span>
    <span>{capability.credential_available ? t('observability-credential-available') : t('observability-credential-unavailable')}</span></div>;
}
function BackendFields({ name, form, disabled, onEdits }: { name: BackendName; form: Form; disabled: boolean; onEdits: EditState }) {
  const prefix = `observability.query.${name}`;
  const label = name === 'tempo' ? 'Tempo' : name === 'loki' ? 'Loki' : 'Prometheus';
  const url = atPath(form, `${prefix}.url`);
  const credential = atPath(form, `${prefix}.credential_env`);
  const clearBackend = () => onEdits(current => ({
    ...Object.fromEntries(Object.entries(current).filter(([path]) => path !== prefix && !path.startsWith(`${prefix}.`))), [prefix]: null,
  }));
  return <fieldset className="observability-settings-backend"><legend>{label}</legend>
    <TextField label={t('observability-settings-backend-url')} value={url} disabled={disabled} nullable required help={t('observability-settings-backend-url-help')}
      onChange={value => { if (value === null) clearBackend(); else onEdits(current => ({ ...current, [`${prefix}.url`]: value })); }} />
    <TextField label={t('observability-settings-credential-env')} value={credential} disabled={disabled} nullable help={t('observability-settings-credential-help')}
      onChange={value => onEdits(current => ({ ...current, [`${prefix}.credential_env`]: value }))} />
  </fieldset>;
}
function TextField({ label, value, onChange, disabled, nullable = false, help, required = false }: {
  label: string; value: Json | undefined; onChange: (value: string | null) => void; disabled: boolean; nullable?: boolean; help?: string; required?: boolean;
}) {
  return <label className="field"><span>{label}{required && ` *`}</span><input type="text" value={value == null ? '' : String(value)} disabled={disabled} autoComplete="off" spellCheck={false}
    onChange={event => onChange(event.target.value || (nullable ? null : ''))} />{help && <small>{help}</small>}</label>;
}
function NumberField({ label, value, onChange, disabled, min, max }: {
  label: string; value: Json | undefined; onChange: (value: number | string) => void; disabled: boolean; min: number; max: number;
}) {
  return <label className="field"><span>{label}</span><input type="number" min={min} max={max} value={value == null ? '' : String(value)} disabled={disabled}
    onChange={event => onChange(event.target.value === '' ? '' : Number(event.target.value))} /></label>;
}
function CheckboxField({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (checked: boolean) => void; disabled: boolean }) {
  return <label className="field field-checkbox"><span>{label}</span><input type="checkbox" checked={checked} disabled={disabled} onChange={event => onChange(event.target.checked)} /></label>;
}
