import { t, message, useLocale } from '../i18n';
import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, errorText, errorValue } from '../bridge';
import { Modal } from './Modal';
import { StorageLocations } from './StorageLocations';

import type { UpdateAction, UpdateStatus } from '../commands';
export type { UpdateStatus } from '../commands';
export function Updates({ epoch, onClose, onChanged }: { epoch: number; onClose: () => void; onChanged: () => void }) {
  useLocale();
  const [status, setStatus] = useState<UpdateStatus>();
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [storage, setStorage] = useState(false);
  const [confirm, setConfirm] = useState<'install' | 'recover' | 'clean_cache' | 'clean_images'>();
  useEffect(() => {
    let alive = true;
    const refresh = () => { void api('update_action', { action: 'status', epoch }).then(value => { if (alive) setStatus(value); }).catch(e => { if (alive) setError(errorValue(e)); }); };
    refresh(); const timer = window.setInterval(refresh, 1000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [epoch]);
  const run = async (action: UpdateAction) => {
    setWorking(true); setError(''); setConfirm(undefined);
    try { setStatus(await api('update_action', { action, epoch })); onChanged(); }
    catch (e) { setError(errorValue(e)); }
    finally { setWorking(false); }
  };
  const linux = navigator.userAgent.includes('Linux');
  return <Modal title={status?.needs_setup ? t('updates-welcome-to-wukong-set-up-environment') : t('workspace-updates-and-environment')} description={t('updates-the-client-and-tools-update-as-a')} wide onClose={() => { if (!working) onClose(); }}>
    <div className="update-summary"><div><span className="muted">{t('updates-current-client')}</span><h3>{status?.version ?? t('updates-loading')}</h3></div><div><span className="muted">{t('updates-available-version')}</span><h3>{status?.available ?? t('updates-no-update-found-yet')}</h3></div></div>
    {status?.needs_setup && <ol className="setup-steps"><li>{t('updates-install-and-start-docker-with-support-for')}</li><li>{t('updates-select-prepare-tool-environment-to-download-and')}</li><li>{t('updates-once-ready-configure-your-model-provider-in')}</li></ol>}
    <div className="header-actions"><button onClick={() => void openUrl(linux ? 'https://docs.docker.com/engine/install/' : 'https://docs.docker.com/desktop/setup/install/windows-install/')}>{t('updates-docker-installation-guide')}</button>{linux && <button disabled={working} onClick={() => void run('integrate')}>{t('updates-add-to-application-menu-and-restart')}</button>}</div>
    <p role="status">{(status?.message && message(status.message)) || t('updates-check-for-updates-or-prepare-the-current')}</p>
    {status?.phase === 'downloading' && status.total && <div><progress aria-label={t('updates-client-download-progress')} max={status.total} value={status.downloaded} /><p className="muted">{(status.downloaded / 1048576).toFixed(1)} / {(status.total / 1048576).toFixed(1)} MiB</p></div>}
    {working && <p className="muted">{t('updates-keep-the-application-open-while-processing-image')}</p>}
    {(error || status?.phase === 'error') && <p className="error-banner" role="alert">{errorText(error) || (status?.message && message(status.message))}</p>}
    {status?.notes && <details><summary>{t('updates-release-notes')}</summary><p>{status.notes}</p></details>}
    <details onToggle={event => setStorage(event.currentTarget.open)}><summary>{t('updates-configure-storage-locations')}</summary>{storage && <StorageLocations epoch={epoch} disabled={working || Boolean(status?.pending)} onChanged={onChanged} />}</details>
    {status?.pending && <p className="error-banner">{t('updates-the-previous-update-is-not-confirmed-new')}</p>}
    <fieldset disabled={working} className="workbench-controls"><legend>{t('updates-version-and-environment')}</legend><div className="header-actions">
      <button onClick={() => void run('check')}>{t('updates-check-for-updates')}</button>
      <button disabled={status?.pending} onClick={() => void run('setup')}>{status?.needs_setup ? t('updates-prepare-tool-environment') : t('updates-validate-and-repair-environment')}</button>
      <button disabled={!status?.available || status.pending || status.needs_setup} onClick={() => void run('prepare')}>{t('updates-download-and-verify-update')}</button>
      <button className="primary" disabled={!status?.prepared || status.pending} onClick={() => setConfirm('install')}>{t('updates-install-and-restart')}</button>
    </div><div className="header-actions"><button disabled={!status?.recovery} onClick={() => setConfirm('recover')}>{t('updates-restore-previous-version')}</button><button disabled={status?.pending} onClick={() => setConfirm('clean_cache')}>{t('updates-clean-old-download-cache')}</button><button disabled={status?.pending} onClick={() => setConfirm('clean_images')}>{t('updates-clean-old-tool-images')}</button></div></fieldset>
    {status?.recovery && <details><summary>{t('updates-recovery-when-the-app-will-not-start')}</summary><p>{t('updates-close-wukong-and-run-this-independent-recovery')}</p><code className="path">{status.recovery}</code></details>}
    {confirm && <div className="notice"><p>{confirm === 'install' ? t('updates-installation-closes-and-restarts-the-client-save') : confirm === 'recover' ? t('updates-the-client-will-exit-and-open-the') : confirm === 'clean_images' ? t('updates-clean-only-old-tool-images-recorded-by') : t('updates-clean-old-client-downloads-the-current-version')}</p><div className="header-actions"><button onClick={() => setConfirm(undefined)}>{t('workspace-cancel')}</button><button className="primary" onClick={() => void run(confirm)}>{t('updates-confirm')}</button></div></div>}
  </Modal>;
}
