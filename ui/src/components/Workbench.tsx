import { t, useLocale } from '../i18n';
import { useState } from 'react';
import { api, errorText } from '../bridge';
import type { WorkbenchRequest } from '../commands';
import { Modal } from './Modal';

export function Workbench({ epoch, onClose, onChanged }: { epoch: number; onClose: () => void; onChanged: () => void }) {
  useLocale();
  const [id, setId] = useState('');
  const [source, setSource] = useState('');
  const [binary, setBinary] = useState('');
  const [destination, setDestination] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<unknown>();
  const [error, setError] = useState('');
  const run = async (request: WorkbenchRequest) => {
    setBusy(true); setError('');
    try { setResult(await api('workbench_action', { epoch, request })); onChanged(); }
    catch (e) { setError(errorText(e)); }
    finally { setBusy(false); }
  };
  return <Modal title={t('updates-pwn-workbench')} description={t('updates-tools-and-challenges-run-separately-each-challenge')} wide onClose={() => { if (!busy) onClose(); }}>
    <fieldset disabled={busy} className="workbench-controls">
      <legend>{t('updates-tool-environment')}</legend>
      <div className="header-actions">
        <button onClick={() => void run({ action: 'status' })}>{t('updates-show-status')}</button>
        <button onClick={() => void run({ action: 'up' })}>{t('updates-start-environment')}</button>
        <button onClick={() => void run({ action: 'doctor' })}>{t('updates-run-diagnostics')}</button>
        <button onClick={() => void run({ action: 'down' })}>{t('updates-stop-environment')}</button>
        <button onClick={() => void run({ action: 'select', id: null })}>{t('updates-use-tool-environment')}</button>
      </div>
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
      <button disabled={!destination} onClick={() => void run({ action: 'export', destination })}>{t('updates-export-offline-migration-package')}</button>
    </fieldset>
    {busy && <p role="status">{t('updates-processing-please-wait-for-completion')}</p>}
    {error && <p className="error-banner" role="alert">{error}</p>}
    {result !== undefined && <details open><summary>{t('updates-operation-result')}</summary><pre className="workbench-result">{JSON.stringify(result, null, 2)}</pre></details>}
  </Modal>;
}
