import { t, useLocale } from '../i18n';
import { useEffect, useState } from 'react';
import { FileCode2, FolderOpen, RotateCcw, Save } from 'lucide-react';
import { api, errorText, errorValue } from '../bridge';
import type { ConfigDocument, ConfigEdit, Host, Json } from '../types';
import { Modal } from './Modal';
import { StorageLocations, type StorageDraft } from './StorageLocations';
import { LanguageSettings } from './LanguageSettings';

type Field = { path: string; label: string; kind?: 'number' | 'checkbox' | 'provider' | 'backend'; help?: string };
const groups = (): { title: string; fields: Field[] }[] => [
  { title: t('settings-model'), fields: [
    { path: 'model.provider', label: t('settings-provider'), kind: 'provider' }, { path: 'model.model', label: t('settings-main-model') },
    { path: 'model.role_model', label: t('settings-collaborator-model'), help: t('settings-leave-blank-to-use-the-main-model') },
    { path: 'model.base_url', label: t('settings-api-endpoint'), help: t('settings-leave-blank-to-use-the-provider-default') },
    { path: 'model.api_key_env', label: t('settings-api-key-environment-variable'), help: t('settings-enter-the-variable-name-the-key-is') },
    { path: 'model.max_tokens', label: t('settings-output-tokens-per-request'), kind: 'number' },
    { path: 'model.request_timeout_secs', label: t('settings-request-timeout-seconds'), kind: 'number' },
  ] },
  { title: t('settings-task-budget'), fields: [
    { path: 'budget.max_total_tokens', label: t('settings-total-tokens'), kind: 'number' },
    { path: 'budget.max_turns_per_agent', label: t('settings-maximum-turns-per-agent'), kind: 'number' },
    { path: 'budget.max_agents', label: t('settings-maximum-agents'), kind: 'number' },
    { path: 'budget.max_concurrent_agents', label: t('settings-concurrent-agents'), kind: 'number' },
    { path: 'budget.max_depth', label: t('settings-maximum-collaboration-depth'), kind: 'number' },
    { path: 'budget.max_wall_seconds', label: t('settings-task-time-limit-seconds'), kind: 'number' },
  ] },
  { title: t('settings-local-environment'), fields: [
    { path: 'analysis.ghidra_executable', label: t('settings-ghidra-executable') },
    { path: 'runtime.backend', label: t('settings-dynamic-analysis-backend'), kind: 'backend' },
    { path: 'runtime.docker_container', label: t('settings-docker-container') },
    { path: 'runtime.ssh_host', label: t('settings-ssh-host') },
    { path: 'runtime.python', label: t('settings-python-executable') },
    { path: 'runtime.bridge_path', label: t('settings-runtime-bridge-path') },
    { path: 'execution.stream', label: t('settings-stream-model-responses'), kind: 'checkbox' },
    { path: 'execution.summarize_tool_outputs', label: t('settings-summarize-large-tool-outputs-in-model-context'), kind: 'checkbox' },
  ] },
];
function atPath(form: Record<string, Json>, path: string): Json | undefined {
  let value: Json | undefined = form;
  for (const key of path.split('.')) value = value && typeof value === 'object' && !Array.isArray(value) ? value[key] : undefined;
  return value;
}
export function Settings({ open, host, onClose, onSaved, onDirty }: {
  open: boolean; host: Host; onClose: () => void; onSaved: () => void; onDirty: (value: boolean) => void;
}) {
  useLocale();
  const [document, setDocument] = useState<ConfigDocument>();
  const [loadedEpoch, setLoadedEpoch] = useState(host.epoch);
  const [edits, setEdits] = useState<Record<string, Json>>({});
  const [section, setSection] = useState<'general' | 'budget' | 'storage' | 'advanced'>('general');
  const [storageVisited, setStorageVisited] = useState(false);
  const [storageBusy, setStorageBusy] = useState(false);
  const [storageDraft, setStorageDraft] = useState<StorageDraft>();
  const storageDirty = Boolean(storageDraft && (storageDraft.data !== storageDraft.initialData || storageDraft.cache !== storageDraft.initialCache));
  const [raw, setRaw] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const dirty = Boolean(document && (document.source !== (document.original ?? '') || Object.keys(edits).length));
  useEffect(() => { onDirty(dirty || storageDirty); }, [dirty, storageDirty, onDirty]);
  useEffect(() => {
    if (!open || document) return;
    let alive = true;
    setBusy(true); setError('');
    void api('config_read', { epoch: host.epoch }).then(value => { if (alive) { setBusy(false); setDocument(value); setLoadedEpoch(host.epoch); } })
      .catch(error => { if (alive) setError(errorValue(error)); }).finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, [open, document, host.epoch]);
  const patch = (): ConfigEdit[] => Object.entries(edits).map(([path, value]) => ({ path: path.split('.'), value }));
  const load = async (choose: boolean) => {
    if (dirty && !window.confirm(t('settings-discard-settings-draft-and-reload'))) return;
    setBusy(true); setError('');
    try {
      const value = await api(choose ? 'config_open' : 'config_read', { epoch: host.epoch });
      if (value) { setDocument(value); setEdits({}); setLoadedEpoch(host.epoch); }
    } catch (error) { setError(errorValue(error)); }
    finally { setBusy(false); }
  };
  const switchMode = async () => {
    if (!document) return;
    setBusy(true); setError('');
    try {
      const value = await api('config_preview', { document, edits: patch() });
      setDocument(value); setEdits({}); setRaw(value => !value);
      return true;
    } catch (error) { setError(errorValue(error)); return false; }
    finally { setBusy(false); }
  };
  const selectSection = async (value: typeof section) => {
    if (raw && value !== 'advanced' && !(await switchMode())) return;
    setSection(value);
    if (value === 'storage') setStorageVisited(true);
  };
  const save = async () => {
    if (!document) return;
    setBusy(true); setError('');
    try {
      const value = await api('config_save', { epoch: loadedEpoch, document, edits: patch() });
      setDocument(value); setEdits({}); onDirty(false); onSaved(); onClose();
      // A later opening reads the current document and workspace epoch.
      setDocument(undefined);
    } catch (error) { setError(errorValue(error)); }
    finally { setBusy(false); }
  };
  if (!open) return null;
  return <Modal title={t('settings-workspace-settings')} description={t('settings-new-tasks-use-the-current-configuration-resumed')} onClose={() => { if (!storageBusy) onClose(); }} wide>
    <nav className="settings-sections" aria-label={t('ui-settings-sections')}>
      {(['general', 'budget', 'storage', 'advanced'] as const).map(value => <button key={value} aria-pressed={section === value} disabled={busy || storageBusy} onClick={() => { void selectSection(value); }}>
        {value === 'general' ? t('ui-general-settings') : value === 'budget' ? t('settings-task-budget') : value === 'storage' ? t('settings-storage-locations') : t('ui-advanced-settings')}
      </button>)}
    </nav>
    <div hidden={section !== 'general'}><LanguageSettings /></div>
    {storageVisited && <div hidden={section !== 'storage'}><StorageLocations onWorking={setStorageBusy} draft={storageDraft} onDraft={setStorageDraft} epoch={host.epoch} disabled={busy || dirty || Boolean(host.execution) || host.pending_operations > 0} onChanged={() => { setDocument(undefined); setStorageVisited(false); setStorageDraft(undefined); onSaved(); onClose(); }} />{dirty && <p className="muted">{t('ui-save-before-storage')}</p>}</div>}
    <div hidden={section !== 'advanced'}><div className="settings-toolbar"><span title={document?.path ?? host.config_path}><FileCode2 size={14} />{document?.path ?? host.config_path}</span>
      <button disabled={busy} onClick={() => { void load(true); }}><FolderOpen size={14} />{t('settings-open-configuration')}</button>
      <button disabled={busy} onClick={() => { void load(false); }}><RotateCcw size={14} />{t('settings-reload')}</button>
      <button disabled={busy || !document} onClick={() => { void switchMode(); }}>{raw ? t('settings-form') : t('settings-toml-source')}</button></div></div>
    {loadedEpoch !== host.epoch && <p className="error" role="alert">{t('settings-workspace-configuration-changed-reload-settings-your-draft')}</p>}
    {error && <p className="error" role="alert">{errorText(error)}</p>}
    <div className="settings-scroll" hidden={section === 'storage'}>{!document ? <p className="muted">{busy ? t('settings-loading-configuration') : t('settings-could-not-load-configuration-please-reload')}</p> : raw && section === 'advanced' ?
      <textarea className="toml-editor" aria-label={t('settings-toml-configuration-source')} spellCheck={false} value={document.source} disabled={busy}
        onChange={event => setDocument({ ...document, source: event.target.value })} /> : groups().map((group, index) => ({ ...group, fields: group.fields.filter((field, fieldIndex) => index === 0 ? section === 'general' ? fieldIndex < 5 : section === 'advanced' && fieldIndex >= 5 : index === 1 ? section === 'budget' : section === 'advanced') })).filter(group => group.fields.length).map(group => <section key={group.fields[0].path} className="settings-group"><h3>{group.title}</h3>
        <div className="settings-grid">{group.fields.map(field => {
          const value = Object.hasOwn(edits, field.path) ? edits[field.path] : atPath(document.form, field.path);
          const change = (value: Json) => setEdits(previous => ({ ...previous, [field.path]: value }));
          return <label key={field.path} className={`field ${field.kind === 'checkbox' ? 'field-checkbox' : ''}`}><span>{field.label}</span>
            {field.kind === 'provider' || field.kind === 'backend' ? <select value={String(value ?? '')} disabled={busy} onChange={event => change(event.target.value)}>{(field.kind === 'provider' ? ['openai', 'openai_responses', 'anthropic', 'ollama'] : ['disabled', 'docker', 'ssh', 'trusted_local']).map(option => <option key={option} value={option}>{option}</option>)}</select> :
              field.kind === 'checkbox' ? <input type="checkbox" checked={Boolean(value)} disabled={busy} onChange={event => change(event.target.checked)} /> :
                <input type={field.kind === 'number' ? 'number' : 'text'} min={field.kind === 'number' ? 1 : undefined} value={value == null ? '' : String(value)} disabled={busy}
                  onChange={event => change(field.kind === 'number' ? event.target.value === '' ? '' : Number(event.target.value) : event.target.value || null)} />}
            {field.help && <small>{field.help}</small>}</label>;
        })}</div></section>)}</div>
    <div className="modal-footer"><span className="muted">{dirty || storageDirty ? t('settings-unsaved-changes') : t('settings-configuration-loaded')} {t('settings-comments-and-relative-paths-are-preserved')}</span>
      <button disabled={storageBusy} onClick={onClose}>{t('workspace-close')}</button>{section !== 'storage' && <button className="primary" disabled={busy || !document || loadedEpoch !== host.epoch || Boolean(host.execution) || host.pending_operations > 0} onClick={() => { void save(); }}><Save size={14} />{busy ? t('settings-processing') : t('settings-save-and-apply')}</button>}</div>
    {(host.execution || host.pending_operations > 0) && <p className="muted">{t('settings-pause-analysis-and-wait-for-active-operations')}</p>}
  </Modal>;
}
