import type { ImagePullProgress as Progress } from '../commands';
import { t } from '../i18n';

export function ImagePullProgress({ value, version, busy }: { value?: Progress | null; version?: string | null; busy: boolean }) {
  if (!version) return null;
  const elapsed = Math.floor((value?.elapsed_ms ?? 0) / 1000);
  const time = `${Math.floor(elapsed / 60)}:${String(elapsed % 60).padStart(2, '0')}`;
  return <section className="image-pull-progress" aria-label={t('updates-image-progress')}>
    <strong>{t('updates-image-version', { version })}</strong>
    {value && <>
      <div className="pull-summary"><span>{value.cached ? t('updates-image-cached') : value.finished ? t('updates-image-pulled') : value.layers.length ? t('updates-image-layers', { ready: value.layers.filter(l => l.state === 'ready').length, total: value.layers.length }) : t('updates-image-waiting')}</span><span className="muted">{t('updates-image-elapsed', { time })}</span></div>
      {busy && !value.finished && <progress aria-label={t('updates-image-progress')} />}
      {busy && !value.finished && <p className="muted">{t('updates-last-layer-change', { seconds: Math.floor((value.quiet_ms ?? 0) / 1000) })}</p>}
      {busy && (value.quiet_ms ?? 0) >= 120000 && <p className="notice" role="status">{t('updates-no-recent-layer-change')}</p>}
      {!value.cached && <p className="muted">{t('updates-image-layer-note')}</p>}
      {value.layers.length > 0 && <details><summary>{t('updates-image-layer-details')}</summary><ul className="pull-layers">{value.layers.map(layer => <li key={layer.id}><code>{layer.id}</code><span>{t(`updates-layer-${layer.state}`)}</span></li>)}</ul></details>}
      {value.truncated && <p className="muted">{t('updates-image-layer-limit')}</p>}
    </>}
  </section>;
}
