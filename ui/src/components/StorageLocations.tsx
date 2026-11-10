import { t, useLocale } from '../i18n';
import { useEffect, useState } from 'react';
import { api, errorText, errorValue } from '../bridge';

export function StorageLocations({ epoch, disabled, onChanged }: { epoch: number; disabled: boolean; onChanged: () => void }) {
  useLocale();
  const [data, setData] = useState('');
  const [cache, setCache] = useState('');
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  useEffect(() => {
    let alive = true;
    void api('update_storage', { epoch }).then(value => {
      if (alive) { setData(value.data_dir ?? ''); setCache(value.cache_dir ?? ''); }
    }).catch(e => { if (alive) setError(errorValue(e)); });
    return () => { alive = false; };
  }, [epoch]);
  const choose = async (set: (path: string) => void) => {
    try { const path = await api('pick_directory'); if (path) set(path); }
    catch (e) { setError(errorValue(e)); }
  };
  const save = async (kind: 'data' | 'cache') => {
    if (kind === 'data' && !window.confirm(t('settings-switch-to-the-selected-task-and-evidence'))) return;
    setWorking(true); setError(''); setMessage('');
    try {
      await api(kind === 'data' ? 'update_data_directory' : 'update_cache_directory', { epoch, destination: kind === 'data' ? data : cache });
      setMessage(kind === 'data' ? t('settings-data-directory-changed-the-original-data-remains') : t('settings-cache-directory-changed-recovery-files-were-copied'));
      onChanged();
    } catch (e) { setError(errorValue(e)); }
    finally { setWorking(false); }
  };
  return <fieldset disabled={disabled || working} className="workbench-controls"><legend>{t('settings-storage-locations')}</legend>
    <label>{t('settings-task-and-evidence-directory')}<input value={data} onChange={e => setData(e.target.value)} /></label>
    <p className="muted">{t('settings-select-an-existing-workspace-or-a-new')}</p>
    <div className="header-actions"><button onClick={() => void choose(setData)}>{t('settings-choose-data-directory')}</button><button disabled={!data} onClick={() => void save('data')}>{t('settings-apply-data-directory')}</button></div>
    <label>{t('settings-update-cache-directory')}<input value={cache} onChange={e => setCache(e.target.value)} /></label>
    <p className="muted">{t('settings-installers-and-image-validation-logs-can-be')}</p>
    <div className="header-actions"><button onClick={() => void choose(setCache)}>{t('settings-choose-cache-directory')}</button><button disabled={!cache} onClick={() => void save('cache')}>{t('settings-apply-cache-directory')}</button></div>
    {working && <p role="status">{t('settings-validating-and-saving-please-wait')}</p>}{message && <p role="status">{message}</p>}{error && <p role="alert" className="error-banner">{errorText(error)}</p>}
  </fieldset>;
}
