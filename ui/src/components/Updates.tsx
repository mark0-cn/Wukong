import { t, message, useLocale } from '../i18n';
import { useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { errorText, errorValue } from '../bridge';
import { displayPath } from '../displayPath';
import { CopyButton } from './CopyButton';
import { SetupProgress } from './SetupProgress';
import { ImagePullProgress } from './ImagePullProgress';
import { useUpdateController, type UpdateController } from '../hooks/useUpdateController';
import { UpdateDiagnostics } from './UpdateDiagnostics';
import { EnvironmentHealth } from './EnvironmentHealth';
import { Modal } from './Modal';

import type { UpdateAction, UpdateStatus } from '../commands';
export type { UpdateStatus } from '../commands';
export function Updates({ epoch, onClose, onChanged, controller, mode = 'updates' }: { epoch: number; onClose: () => void; onChanged: () => void; controller?: UpdateController; mode?: 'updates' | 'tools' }) {
  useLocale();
  const local = useUpdateController(epoch, onChanged, !controller);
  const { status, working: busy, operationError, refreshError, run: execute, cancel } = controller ?? local;
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [confirm, setConfirm] = useState<'install' | 'recover' | 'clean_cache' | 'clean_images'>();
  const run = async (action: UpdateAction) => { setError(''); setConfirm(undefined); await execute(action); };
  const failure = error || operationError || (status?.phase === 'error' ? status.message : '');
  const failureTitle = failure && typeof failure === 'object' ? message({ ...failure, detail: undefined }) : errorText(failure);
  const detail = failure && typeof failure === 'object' ? failure.detail : '';
  const linux = navigator.userAgent.includes('Linux');
  return <Modal className={`updates-modal ${busy ? 'updates-running' : ''}`} title={mode === 'tools' ? t('ui-tools-environment') : status?.needs_setup ? t('updates-welcome-to-wukong-set-up-environment') : t('ui-updates')} description={status?.needs_setup ? t('updates-setup-description') : t('updates-the-client-and-tools-update-as-a')} wide onClose={onClose}>
    <div className="updates-panel">
    {refreshError && <p className="notice" role="status">{t('updates-refresh-failed')} {errorText(refreshError)}</p>}
    <div className="update-summary">
      <div><span className="muted">{t('updates-current-client')}</span><strong>{status?.version ?? t('updates-loading')}</strong></div>
      {!status?.needs_setup && <div><span className="muted">{t('updates-available-version')}</span><strong>{status?.available ?? t('updates-no-update-found-yet')}</strong></div>}
      <div><span className="muted">{t('ui-tools-environment')}</span><strong>{!status ? t('updates-loading') : status.needs_setup ? busy ? t('updates-setup-working') : failure ? t('updates-setup-failed') : t('updates-setup-required') : t('updates-health-configured')}</strong></div>
    </div>
    {status && !status.needs_setup && <EnvironmentHealth health={status?.health} />}
    {status?.needs_setup && <SetupProgress step={status.environment_step} busy={busy} failed={Boolean(failure)} />}
    <ImagePullProgress value={status?.image_progress} version={status?.image_version} busy={status?.phase === 'pulling'} />
    {!failure && <p className="update-status" role="status">{busy && status?.message ? message(status.message) : status?.needs_setup && status.phase !== 'cancelled' ? t('updates-setup-ready-to-start') : (status?.message && message(status.message)) || t('updates-check-for-updates-or-prepare-the-current')}</p>}
    {status?.phase === 'downloading' && status.total && <div><progress aria-label={t('updates-client-download-progress')} max={status.total} value={status.downloaded} /><p className="muted">{(status.downloaded / 1048576).toFixed(1)} / {(status.total / 1048576).toFixed(1)} MiB</p></div>}
    {busy && <p className="muted">{t('updates-keep-the-application-open-while-processing-image')}</p>}
    {failure && <section className="update-error">
      <p className="error-banner" role="alert">{failureTitle}</p>
      {detail && <details><summary>{t('updates-diagnostic-output')}</summary><pre className="workbench-result">{detail}</pre></details>}
      <CopyButton value={errorText(failure)} label={t('updates-copy-error')} />
    </section>}
    {status?.operation_id && (status.diagnosis || status.log_available) && <UpdateDiagnostics key={status.operation_id} status={status} retry={() => void run(status.action === 'prepare' ? 'prepare' : status.action === 'check' ? 'check' : status.action === 'health' || status.action === 'doctor' ? status.action : 'setup')} />}
    {!status?.needs_setup && status?.notes && <details><summary>{t('updates-release-notes')}</summary><p>{status.notes}</p></details>}

    {status?.pending && <p className="error-banner">{t('updates-the-previous-update-is-not-confirmed-new')}</p>}
    {status?.recovery && <details><summary>{t('updates-recovery-when-the-app-will-not-start')}</summary><p>{t('updates-close-wukong-and-run-this-independent-recovery')}</p><code className="path">{displayPath(status.recovery)}</code></details>}
    </div>
    <div className="updates-footer">
    {busy && <div className="background-controls"><button onClick={onClose}>{t('updates-background')}</button><button disabled={!status?.cancellable || status.cancelling} onClick={() => void cancel()}>{status?.cancelling ? t('updates-cancelling') : t('updates-cancel-preparation')}</button>{!status?.cancellable && <p className="muted">{t('updates-critical-phase')}</p>}</div>}
    <fieldset disabled={busy || !status} className="workbench-controls"><legend className="sr-only">{t('updates-version-and-environment')}</legend>
      <div className="header-actions">
        {mode === 'tools' ? <button className="primary" disabled={status?.pending} onClick={() => void run('setup')}>{t('updates-validate-and-repair-environment')}</button> : status?.needs_setup ? <button className="primary" disabled={status.pending} onClick={() => void run('setup')}>{failure ? t('updates-retry-setup') : t('updates-prepare-tool-environment')}</button>
          : status?.prepared ? <button className="primary" disabled={status.pending} onClick={() => setConfirm('install')}>{t('updates-install-and-restart')}</button>
          : status?.available ? <button className="primary" disabled={status.pending} onClick={() => void run('prepare')}>{t('updates-download-and-verify-update')}</button>
          : <button className="primary" onClick={() => void run('check')}>{t('updates-check-for-updates')}</button>}
        {(mode !== 'tools' && !status?.needs_setup && (status?.available || status?.prepared)) && <button onClick={() => void run('check')}>{t('updates-check-for-updates')}</button>}
        <button onClick={() => void openUrl(linux ? 'https://docs.docker.com/engine/install/' : 'https://docs.docker.com/desktop/setup/install/windows-install/').catch(e => setError(errorValue(e)))}>{t('updates-docker-installation-guide')}</button>
      </div>
      {mode === 'tools' && <div className="header-actions"><button onClick={() => void run('start_tools')}>{t('updates-start-environment')}</button><button onClick={() => void run('stop_tools')}>{t('updates-stop-environment')}</button><button onClick={() => void run('select_tools')}>{t('updates-use-tool-environment')}</button></div>}
      {!status?.needs_setup && <div className="header-actions"><button onClick={() => void run('health')}>{t('updates-health-quick')}</button><button onClick={() => void run('doctor')}>{t('updates-health-full')}</button></div>}
      {(!status?.needs_setup || status?.pending) && <details className="maintenance-options" open={status?.pending || undefined}><summary>{t('ui-maintenance-options')}</summary><div className="header-actions">
        <button disabled={!status?.recovery} onClick={() => setConfirm('recover')}>{t('updates-restore-previous-version')}</button>
        <button disabled={status?.pending} onClick={() => setConfirm('clean_cache')}>{t('updates-clean-old-download-cache')}</button>
        <button disabled={status?.pending} onClick={() => setConfirm('clean_images')}>{t('updates-clean-old-tool-images')}</button>
        {linux && <button onClick={() => void run('integrate')}>{t('updates-add-to-application-menu-and-restart')}</button>}
      </div></details>}
    </fieldset>
    {confirm && <div className="notice"><p>{confirm === 'install' ? t('updates-installation-closes-and-restarts-the-client-save') : confirm === 'recover' ? t('updates-the-client-will-exit-and-open-the') : confirm === 'clean_images' ? t('updates-clean-only-old-tool-images-recorded-by') : t('updates-clean-old-client-downloads-the-current-version')}</p><div className="header-actions"><button onClick={() => setConfirm(undefined)}>{t('workspace-cancel')}</button><button className="primary" onClick={() => void run(confirm)}>{t('updates-confirm')}</button></div></div>}
    </div>
  </Modal>;
}
