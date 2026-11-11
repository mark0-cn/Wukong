import { t, message, useLocale } from '../i18n';
import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, errorText, errorValue } from '../bridge';
import { displayPath } from '../displayPath';
import { CopyButton } from './CopyButton';
import { SetupProgress } from './SetupProgress';
import { Modal } from './Modal';

import type { UpdateAction, UpdateStatus } from '../commands';
export type { UpdateStatus } from '../commands';
export function Updates({ epoch, onClose, onChanged }: { epoch: number; onClose: () => void; onChanged: () => void }) {
  useLocale();
  const [status, setStatus] = useState<UpdateStatus>();
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [confirm, setConfirm] = useState<'install' | 'recover' | 'clean_cache' | 'clean_images'>();
  useEffect(() => {
    let alive = true;
    const refresh = () => { void api('update_action', { action: 'status', epoch }).then(value => { if (alive) setStatus(value); }).catch(e => { if (alive) setError(errorValue(e)); }); };
    refresh(); const timer = window.setInterval(refresh, 1000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [epoch]);
  const run = async (action: UpdateAction) => {
    setWorking(true); setError(''); setConfirm(undefined);
    setStatus(previous => previous && { ...previous, phase: 'starting', message: '' });
    try { setStatus(await api('update_action', { action, epoch })); onChanged(); }
    catch (e) { setError(errorValue(e)); }
    finally { setWorking(false); }
  };
  const busy = working || ['starting', 'checking', 'downloading', 'environment', 'docker', 'pulling', 'validating', 'activating', 'installing', 'storage'].includes(status?.phase ?? '');
  const failure = error || (status?.phase === 'error' ? status.message : '');
  const failureTitle = failure && typeof failure === 'object' ? message({ ...failure, detail: undefined }) : errorText(failure);
  const detail = failure && typeof failure === 'object' ? failure.detail : '';
  const linux = navigator.userAgent.includes('Linux');
  return <Modal className="updates-modal" title={status?.needs_setup ? t('updates-welcome-to-wukong-set-up-environment') : t('ui-updates')} description={status?.needs_setup ? t('updates-setup-description') : t('updates-the-client-and-tools-update-as-a')} wide onClose={() => { if (!busy) onClose(); }}>
    <div className="updates-panel">
    <div className="update-summary">
      <div><span className="muted">{t('updates-current-client')}</span><strong>{status?.version ?? t('updates-loading')}</strong></div>
      {!status?.needs_setup && <div><span className="muted">{t('updates-available-version')}</span><strong>{status?.available ?? t('updates-no-update-found-yet')}</strong></div>}
      <div><span className="muted">{t('ui-tools-environment')}</span><strong>{!status ? t('updates-loading') : status.needs_setup ? busy ? t('updates-setup-working') : failure ? t('updates-setup-failed') : t('updates-setup-required') : t('updates-environment-ready')}</strong></div>
    </div>
    {status?.needs_setup && <SetupProgress step={status.environment_step} busy={busy} failed={Boolean(failure)} />}
    {!failure && <p className="update-status" role="status">{busy && status?.message ? message(status.message) : status?.needs_setup ? t('updates-setup-ready-to-start') : (status?.message && message(status.message)) || t('updates-check-for-updates-or-prepare-the-current')}</p>}
    {status?.phase === 'downloading' && status.total && <div><progress aria-label={t('updates-client-download-progress')} max={status.total} value={status.downloaded} /><p className="muted">{(status.downloaded / 1048576).toFixed(1)} / {(status.total / 1048576).toFixed(1)} MiB</p></div>}
    {busy && <p className="muted">{t('updates-keep-the-application-open-while-processing-image')}</p>}
    {failure && <section className="update-error">
      <p className="error-banner" role="alert">{failureTitle}</p>
      {detail && <details><summary>{t('updates-diagnostic-output')}</summary><pre className="workbench-result">{detail}</pre></details>}
      <CopyButton value={errorText(failure)} label={t('updates-copy-error')} />
    </section>}
    {!status?.needs_setup && status?.notes && <details><summary>{t('updates-release-notes')}</summary><p>{status.notes}</p></details>}

    {status?.pending && <p className="error-banner">{t('updates-the-previous-update-is-not-confirmed-new')}</p>}
    <fieldset disabled={busy || !status} className="workbench-controls"><legend className="sr-only">{t('updates-version-and-environment')}</legend>
      <div className="header-actions">
        {status?.needs_setup ? <button className="primary" disabled={status.pending} onClick={() => void run('setup')}>{failure ? t('updates-retry-setup') : t('updates-prepare-tool-environment')}</button>
          : status?.prepared ? <button className="primary" disabled={status.pending} onClick={() => setConfirm('install')}>{t('updates-install-and-restart')}</button>
          : status?.available ? <button className="primary" disabled={status.pending} onClick={() => void run('prepare')}>{t('updates-download-and-verify-update')}</button>
          : <button className="primary" onClick={() => void run('check')}>{t('updates-check-for-updates')}</button>}
        {(!status?.needs_setup && (status?.available || status?.prepared)) && <button onClick={() => void run('check')}>{t('updates-check-for-updates')}</button>}
        <button onClick={() => void openUrl(linux ? 'https://docs.docker.com/engine/install/' : 'https://docs.docker.com/desktop/setup/install/windows-install/').catch(e => setError(errorValue(e)))}>{t('updates-docker-installation-guide')}</button>
      </div>
      {(!status?.needs_setup || status?.pending) && <details className="maintenance-options" open={status?.pending || undefined}><summary>{t('ui-maintenance-options')}</summary><div className="header-actions">
        <button disabled={!status?.recovery} onClick={() => setConfirm('recover')}>{t('updates-restore-previous-version')}</button>
        <button disabled={status?.pending} onClick={() => setConfirm('clean_cache')}>{t('updates-clean-old-download-cache')}</button>
        <button disabled={status?.pending} onClick={() => setConfirm('clean_images')}>{t('updates-clean-old-tool-images')}</button>
        {linux && <button onClick={() => void run('integrate')}>{t('updates-add-to-application-menu-and-restart')}</button>}
      </div></details>}
    </fieldset>
    {status?.recovery && <details><summary>{t('updates-recovery-when-the-app-will-not-start')}</summary><p>{t('updates-close-wukong-and-run-this-independent-recovery')}</p><code className="path">{displayPath(status.recovery)}</code></details>}
    {confirm && <div className="notice"><p>{confirm === 'install' ? t('updates-installation-closes-and-restarts-the-client-save') : confirm === 'recover' ? t('updates-the-client-will-exit-and-open-the') : confirm === 'clean_images' ? t('updates-clean-only-old-tool-images-recorded-by') : t('updates-clean-old-client-downloads-the-current-version')}</p><div className="header-actions"><button onClick={() => setConfirm(undefined)}>{t('workspace-cancel')}</button><button className="primary" onClick={() => void run(confirm)}>{t('updates-confirm')}</button></div></div>}
    </div>
  </Modal>;
}
