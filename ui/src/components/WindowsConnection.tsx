import { useId, useRef, useState, type Dispatch, type SetStateAction } from 'react';
import { api, errorText, errorValue } from '../bridge';
import { t } from '../i18n';
import type { WindowsConnectionInput } from '../commands';

export interface WindowsConnectionDraft {
  receipt: string; name: string; host: string; identity: string; fingerprint: string; makeDefault: boolean | null;
}
export const emptyWindowsConnection = (): WindowsConnectionDraft => ({ receipt: '', name: '', host: '', identity: '', fingerprint: '', makeDefault: null });
export function WindowsConnection({ disabled, hasEnvironments, draft, onDraft, onAdd }: {
  disabled: boolean; hasEnvironments: boolean;
  draft: WindowsConnectionDraft; onDraft: Dispatch<SetStateAction<WindowsConnectionDraft>>;
  onAdd: (input: WindowsConnectionInput) => Promise<void>;
}) {
  const fieldId = useId();
  const receiptInput = useRef<HTMLInputElement>(null);
  const { receipt, name, host, identity, fingerprint } = draft;
  const makeDefault = draft.makeDefault ?? !hasEnvironments;
  const setField = <K extends keyof WindowsConnectionDraft>(key: K, value: WindowsConnectionDraft[K]) => onDraft(previous => ({ ...previous, [key]: value }));
  const setReceipt = (value: string) => setField('receipt', value);
  const setName = (value: string) => setField('name', value);
  const setHost = (value: string) => setField('host', value);
  const setIdentity = (value: string) => setField('identity', value);
  const setFingerprint = (value: string) => setField('fingerprint', value);
  const [working, setWorking] = useState(false);
  const [added, setAdded] = useState(false);
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  let publicFingerprint = '';
  try {
    const value = JSON.parse(receipt);
    if (typeof value?.host_fingerprint === 'string') publicFingerprint = value.host_fingerprint;
  } catch { /* A partial pasted receipt is still an editable draft. */ }
  const matches = Boolean(publicFingerprint && fingerprint.trim() === publicFingerprint);
  const ready = !disabled && !working && Boolean(receipt && host.trim() && identity && matches);
  const chooseIdentity = async () => {
    setError(''); setWorking(true);
    try { const path = await api('pick_ssh_identity'); if (path) setIdentity(path); }
    catch (error) { setError(errorValue(error)); }
    finally { setWorking(false); }
  };
  const submit = async () => {
    if (!ready) return;
    setError(''); setWorking(true); setAdded(false);
    try {
      await onAdd({ connection_json: receipt, name: name.trim() || t('connection-default-name'), host: host.trim(),
        identity_file: identity, confirmed_fingerprint: fingerprint.trim(), make_default: makeDefault });
      onDraft(emptyWindowsConnection()); setAdded(true);
      if (receiptInput.current) receiptInput.current.value = '';
    } catch (error) { setError(errorValue(error)); }
    finally { setWorking(false); }
  };
  return <section className="settings-group" aria-label={t('connection-title')}>
    <h3>{t('connection-title')}</h3><p className="muted">{t('connection-description')}</p>
    {added ? <><p role="status">{t('connection-added-draft')}</p><button type="button" onClick={() => setAdded(false)}>{t('connection-add-another')}</button></> : <>
    <fieldset disabled={disabled || working} className="connection-fields"><legend className="sr-only">{t('connection-title')}</legend>
      <div className="field"><span>{t('connection-receipt-file')}</span><input ref={receiptInput} className="sr-only" aria-label={t('connection-receipt-file')} type="file" accept=".json,application/json" onChange={event => {
        const file = event.target.files?.[0]; if (!file) return;
        setError(''); setAdded(false); setReceipt(''); setFingerprint('');
        if (file.size > 16384) { setError(t('connection-receipt-too-large')); return; }
        setWorking(true);
        void file.text().then(setReceipt).catch(error => setError(errorValue(error))).finally(() => setWorking(false));
      }} /><span className="connection-key-picker"><button type="button" onClick={() => receiptInput.current?.click()}>{t('connection-choose-record')}</button><small>{receipt ? t('connection-record-loaded') : t('connection-record-pending')}</small></span></div>
      <details><summary>{t('connection-paste-receipt')}</summary><label className="field"><span>{t('connection-public-json')}</span>
        <textarea className="connection-json" spellCheck={false} maxLength={16384} value={receipt} onChange={event => { setReceipt(event.target.value); setFingerprint(''); setAdded(false); }} /></label></details>
      <div className="settings-grid">
        <label className="field"><span>{t('settings-environment-name')}</span><input value={name} placeholder={t('connection-default-name')} onChange={event => setName(event.target.value)} /></label>
        <label className="field"><span>{t('connection-address')}</span><input value={host} autoComplete="off" placeholder="192.0.2.1" onChange={event => setHost(event.target.value)} /></label>
      </div>
      <div className="field"><label htmlFor={`${fieldId}-identity`}>{t('connection-private-key')}</label><span className="connection-key-picker"><input id={`${fieldId}-identity`} aria-describedby={`${fieldId}-identity-help`} value={identity} autoComplete="off" onChange={event => setIdentity(event.target.value)} /><button type="button" onClick={() => void chooseIdentity()}>{t('connection-choose-key')}</button></span><small id={`${fieldId}-identity-help`}>{t('connection-private-key-help')}</small></div>
      {publicFingerprint && <p className="connection-fingerprint"><span>{t('connection-receipt-fingerprint')}</span><code>{publicFingerprint}</code></p>}
      <div className="field"><label htmlFor={`${fieldId}-fingerprint`}>{t('connection-confirm-fingerprint')}</label><input id={`${fieldId}-fingerprint`} aria-describedby={`${fieldId}-fingerprint-help`} value={fingerprint} spellCheck={false} autoComplete="off" placeholder="SHA256:…" onChange={event => setFingerprint(event.target.value)} /><small id={`${fieldId}-fingerprint-help`}>{t('connection-fingerprint-help')}</small></div>
      {fingerprint && !matches && <p className="error" role="alert">{t('connection-fingerprint-mismatch')}</p>}
      <label className="connection-default"><input type="checkbox" disabled={!hasEnvironments} checked={makeDefault} onChange={event => setField('makeDefault', event.target.checked)} /><span>{t('connection-make-default')}</span></label>
      <button type="button" disabled={!ready} onClick={() => void submit()}>{working ? t('settings-processing') : t('connection-add-draft')}</button>
      <button type="button" onClick={() => { onDraft(emptyWindowsConnection()); setError(''); if (receiptInput.current) receiptInput.current.value = ''; }}>{t('connection-reset')}</button>
    </fieldset>
    {error && <p className="error" role="alert">{errorText(error)}</p>}
    </>}
  </section>;
}
