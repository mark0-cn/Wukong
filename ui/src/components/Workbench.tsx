import { t, useLocale } from '../i18n';
import { useState } from 'react';
import { api, errorText, errorValue } from '../bridge';
import type { WorkbenchRequest } from '../commands';
import { Modal } from './Modal';

export function Workbench({ mode, epoch, onClose, onChanged }: { mode: 'tools' | 'challenges'; epoch: number; onClose: () => void; onChanged: () => void }) {
  useLocale();
  const [id, setId] = useState('');
  const [source, setSource] = useState('');
  const [binary, setBinary] = useState('');
  const [destination, setDestination] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<unknown>();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const run = async (request: WorkbenchRequest) => {
    setBusy(true); setError('');
    try { setResult(await api('workbench_action', { epoch, request })); onChanged(); }
    catch (e) { setError(errorValue(e)); }
    finally { setBusy(false); }
  };
  const prepareTools = async () => {
    setBusy(true); setError('');
    try { setResult(await api('update_action', { epoch, action: 'setup' })); onChanged(); }
    catch (e) { setError(errorValue(e)); } finally { setBusy(false); }
  };
  return <Modal title={mode === 'tools' ? t('ui-tools-environment') : t('ui-challenge-environment')} description={t('updates-tools-and-challenges-run-separately-each-challenge')} wide onClose={() => { if (!busy) onClose(); }}>
    <fieldset disabled={busy} className="workbench-controls">
      <legend className="sr-only">{mode === 'tools' ? t('ui-tools-environment') : t('ui-challenge-environment')}</legend>
      {mode === 'tools' ? <><p className="muted">{t('ui-tools-description')}</p><button className="primary" onClick={() => void prepareTools()}>{t('updates-validate-and-repair-environment')}</button>
      <div className="header-actions">
        <button onClick={() => void run({ action: 'status' })}>{t('updates-show-status')}</button>
        <button onClick={() => void run({ action: 'up' })}>{t('updates-start-environment')}</button>
        <button onClick={() => void run({ action: 'doctor' })}>{t('updates-run-diagnostics')}</button>
        <button onClick={() => void run({ action: 'down' })}>{t('updates-stop-environment')}</button>
        <button onClick={() => void run({ action: 'select', id: null })}>{t('updates-use-tool-environment')}</button>
      </div>
      </> : <>
      <label>{t('updates-challenge-name')}<input value={id} onChange={e => setId(e.target.value)} placeholder={t('updates-for-example-heap-demo')} /></label>
      <label>{t('updates-attachment-directory-or-file')}<input value={source} onChange={e => setSource(e.target.value)} placeholder={t('updates-full-attachment-path')} /></label>
      <label>{t('updates-main-executable-relative-path-if-there-are')}<input value={binary} onChange={e => setBinary(e.target.value)} placeholder={t('updates-for-example-bin-challenge')} /></label>
      <div className="header-actions">
        <button onClick={() => void run({ action: 'list' })}>{t('updates-list-challenges')}</button>
        <button disabled={!id || !source} onClick={() => void run({ action: 'import', id, source, binary: binary || null })}>{t('updates-import-challenge')}</button>
        <button disabled={!id} onClick={() => void run({ action: 'build', id })}>{t('updates-build-challenge-image')}</button>
        <button disabled={!id} onClick={() => void run({ action: 'prepare', id })}>{t('updates-prepare-runtime-files')}</button>
        <button disabled={!id} onClick={() => void run({ action: 'start', id })}>{t('updates-start-challenge')}</button>
        <button disabled={!id} onClick={() => void run({ action: 'stop', id })}>{t('updates-stop-challenge')}</button>
        <button disabled={!id} onClick={() => void run({ action: 'select', id })}>{t('updates-use-this-challenge-for-new-tasks')}</button>
      </div>
      <p className="muted">{t('updates-connect-to-challenge-port-for-service-i')}</p>
      <label>{t('updates-migration-package-directory')}<input value={destination} onChange={e => setDestination(e.target.value)} placeholder={t('updates-enter-a-new-directory-that-does-not')} /></label>
      <button disabled={!destination} onClick={() => void run({ action: 'export', destination })}>{t('updates-export-offline-migration-package')}</button></>}
    </fieldset>
    {busy && <p role="status">{t('updates-processing-please-wait-for-completion')}</p>}
    {error && <p className="error-banner" role="alert">{errorText(error)}</p>}
    {result !== undefined && <details open><summary>{t('updates-operation-result')}</summary><pre className="workbench-result">{JSON.stringify(result, null, 2)}</pre></details>}
  </Modal>;
}
