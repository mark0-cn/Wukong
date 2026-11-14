import { t, message, currentLocale } from '../i18n';
import type { EnvironmentHealth as Health } from '../commands';

export function EnvironmentHealth({ health: observation, epoch }: { health?: Health; epoch?: number }) {
  const health = epoch !== undefined && observation?.checked_at && observation.epoch !== epoch ? undefined : observation;
  const label = (value: boolean | null | undefined) => value === true ? t('updates-health-pass') : value === false ? t('updates-health-fail') : t('updates-health-unknown');
  return <section className="environment-health" aria-label={t('updates-health-title')}>
    <h3>{t('updates-health-title')}</h3>
    <div className="health-grid">
      <div><span>{t('updates-health-config')}</span><strong>{!health ? t('updates-health-unknown') : health.configured ? t('updates-health-configured') : t('updates-health-unconfigured')}</strong></div>
      {health?.platform !== 'windows' && <div><span>{t('updates-health-docker')}</span><strong>{label(health?.docker)}</strong></div>}
      <div><span>{t(health?.platform === 'windows' ? 'updates-health-worker' : 'updates-health-container')}</span><strong>{label(health?.running)}</strong></div>
      <div><span>{t('updates-health-tools')}</span><strong>{label(health?.tools)}</strong></div>
    </div>
    <p className="muted">{health?.checked_at ? t('updates-health-time', { time: new Date(health.checked_at).toLocaleString(currentLocale()) }) : t('updates-health-unchecked-help')}</p>
    {health?.tools_checked_at && <p className="muted">{t('updates-health-tool-time', { time: new Date(health.tools_checked_at).toLocaleString(currentLocale()) })}</p>}
    {health?.error && <p className="error-banner" role="alert">{message(health.error)}</p>}
    {health && <details><summary>{t('updates-technical-details')}</summary><pre className="workbench-result">{JSON.stringify(health, null, 2)}</pre></details>}
  </section>;
}
