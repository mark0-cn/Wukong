import { useEffect, useState } from 'react';
import { api, errorText } from '../bridge';
import { t, useLocale } from '../i18n';
import { displayPath } from '../displayPath';
import type { CleanupPreview } from '../commands';
import { Modal } from './Modal';
import './cleanup.css';

export function CleanupReview({ initialData = false }: {initialData?: boolean}) {
  useLocale();
  const [selection, setSelection] = useState({data: initialData, tools: false, challenges: false});
  const [preview, setPreview] = useState<CleanupPreview>();
  const [error, setError] = useState('');
  const [revision, refresh] = useState(0);
  const [confirming, setConfirming] = useState(false);
  useEffect(() => {
    let active = true;
    setPreview(undefined); setError('');
    void api('cleanup_preview', {selection}).then(value => { if (active) setPreview(value); })
      .catch(reason => { if (active) setError(errorText(reason)); });
    return () => {active = false;};
  }, [selection, revision]);
  const cancel = () => { void api('cleanup_cancel').catch(reason => setError(errorText(reason))); };
  const confirm = async () => {
    if (!preview) return;
    setConfirming(true);
    try { await api('cleanup_confirm', {fingerprint: preview.fingerprint}); }
    catch (reason) { setError(errorText(reason)); setConfirming(false); }
  };
  return <Modal title={t('cleanup-title')} description={t('cleanup-intro')} onClose={cancel} className="cleanup-review">
    <div className="cleanup-body">
      <fieldset disabled={confirming}><legend>{t('cleanup-options')}</legend>
        {(['data', 'tools', 'challenges'] as const).map(key => <label key={key}>
          <input type="checkbox" checked={selection[key]} onChange={event => {setPreview(undefined); setSelection(previous => ({...previous, [key]: event.target.checked}));}} />
          <span><strong>{t(`cleanup-${key}`)}</strong><small>{t(`cleanup-${key}-detail`)}</small></span>
        </label>)}
      </fieldset>
      <p className="muted">{t('cleanup-preserved')}</p>
      <section aria-live="polite" aria-busy={!preview && !error}>
        {!preview && !error && <p role="status">{t('cleanup-inspecting')}</p>}
        {preview && <>
          <h3>{t('cleanup-preview')}</h3>
          <p>{t('cleanup-summary', {files: preview.files, size: (preview.bytes / 1024 / 1024).toFixed(1), resources: preview.resources.length})}</p>
          <details open><summary>{t('cleanup-paths')}</summary><ul>{preview.paths.map(path => <li key={path}><code>{displayPath(path)}</code></li>)}{preview.resources.map(resource => <li key={resource.kind + resource.id}>{resource.kind}: <code>{resource.name}</code></li>)}</ul></details>
          {!!preview.retained.length && <details><summary>{t('cleanup-retained')}</summary><ul>{preview.retained.map((item, i) => <li key={i}><code>{displayPath(item.path)}</code><p className="muted">{item.reason}</p></li>)}</ul></details>}
          {preview.truncated && <p>{t('cleanup-truncated')}</p>}
          {preview.errors.map((message, i) => <p role="alert" key={i}>{message}</p>)}
        </>}
        {error && <p role="alert">{error}</p>}
      </section>
    </div>
    <footer className="cleanup-actions"><button onClick={cancel} disabled={confirming}>{t('cleanup-cancel')}</button><button onClick={() => refresh(value => value + 1)} disabled={confirming}>{t('cleanup-refresh')}</button><button className="primary" onClick={() => void confirm()} disabled={confirming || !preview || !!preview.errors.length || !!error}>{t('cleanup-confirm')}</button></footer>
  </Modal>;
}
