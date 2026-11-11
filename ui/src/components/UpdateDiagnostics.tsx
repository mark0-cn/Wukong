import { useState } from 'react';
import { api, errorText, errorValue } from '../bridge';
import { t } from '../i18n';
import type { UpdateStatus } from '../commands';
import { displayPath } from '../displayPath';

export function UpdateDiagnostics({ status, retry }: { status: UpdateStatus; retry: () => void }) {
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [saved, setSaved] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const hints = {
    daemon: t('updates-help-daemon'), credentials: t('updates-help-credentials'),
    network: t('updates-help-network'), rate_limit: t('updates-help-rate-limit'),
    disk: t('updates-help-disk'), unknown: t('updates-help-unknown'),
  };
  const operate = async (exporting: boolean) => {
    if (!status.operation_id) return;
    setPending(true); setError('');
    try {
      if (exporting) setSaved(await api('update_diagnostics', { operationId: status.operation_id }));
      else await api('update_log_directory', { operationId: status.operation_id });
    } catch (error) { setError(errorValue(error)); }
    finally { setPending(false); }
  };
  return <section className="update-diagnostics" aria-label={t('updates-diagnostics-title')}>
    {status.diagnosis && <p>{hints[status.diagnosis]}</p>}
    <div className="header-actions">
      {status.diagnosis && <button disabled={status.busy || pending} onClick={retry}>{t('updates-retry-operation')}</button>}
      <button disabled={!status.log_available || pending} onClick={() => void operate(false)}>{t('updates-open-log-directory')}</button>
      <button disabled={!status.operation_id || pending} onClick={() => void operate(true)}>{t('updates-export-diagnostics')}</button>
    </div>
    <p className="muted">{t('updates-diagnostics-redaction')}</p>
    {saved && <p role="status">{t('updates-diagnostics-saved')} <span className="path">{displayPath(saved)}</span></p>}
    {error && <p className="error-banner" role="alert">{errorText(error)}</p>}
  </section>;
}
