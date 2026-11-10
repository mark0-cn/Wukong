import { t, useLocale } from '../i18n';
import { useState } from 'react';
import { FolderOpen, Play } from 'lucide-react';
import { api, errorText, errorValue } from '../bridge';
import type { Budget, Run } from '../types';
import { Modal } from './Modal';
import { fileName } from './Status';

export const defaultTask = () => t('workspace-reverse-engineer-the-target-binary-identify-and');
export function NewTask({ target, task, setTarget, setTask, onClose, onStart, busy }: {
  target: string; task: string; setTarget: (value: string) => void; setTask: (value: string) => void;
  onClose: () => void; onStart: () => Promise<void>; busy: boolean;
}) {
  useLocale();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [pending, setPending] = useState(false);
  const pick = async () => {
    try { const path = await api('pick_target'); if (path) setTarget(path); }
    catch (error) { setError(errorValue(error)); }
  };
  return <Modal title={t('workspace-start-a-new-analysis-2')} description={t('workspace-choose-a-local-target-and-describe-what')} onClose={onClose}>
    <label className="field"><span>{t('workspace-target-file')}</span><div className="input-action"><input autoFocus aria-label={t('workspace-target-file-path')} value={target} placeholder={t('workspace-choose-or-drop-a-binary-file')} onChange={event => setTarget(event.target.value)} /><button onClick={() => { void pick(); }} aria-label={t('workspace-choose-target-file')}><FolderOpen size={17} /></button></div></label>
    <label className="field"><span>{t('workspace-analysis-task')}</span><textarea aria-label={t('workspace-analysis-task')} rows={6} value={task} onChange={event => setTask(event.target.value)} /></label>
    {error && <p className="error" role="alert">{errorText(error)}</p>}
    <div className="modal-footer"><span className="muted">{t('workspace-files-and-evidence-stay-in-this-workspace')}</span><button onClick={onClose}>{t('workspace-cancel')}</button>
      <button className="primary" disabled={!target.trim() || !task.trim() || pending || busy} onClick={() => {
        setPending(true); setError(''); void onStart().catch(error => setError(errorValue(error))).finally(() => setPending(false));
      }}><Play size={14} />{pending ? t('workspace-preparing') : t('workspace-start-analysis')}</button></div>
    {busy && <p className="muted">{t('workspace-current-work-has-not-finished-saving-pause')}</p>}
  </Modal>;
}
export function ResumeTask({ run, onClose, onResume }: { run: Run; onClose: () => void; onResume: (budget: Budget) => Promise<void> }) {
  useLocale();
  const [budget, setBudget] = useState<Budget>({ ...run.budget });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const fields: [keyof Budget, string][] = [['max_total_tokens', t('workspace-total-token-limit')], ['max_turns_per_agent', t('workspace-turns-per-agent')], ['max_wall_seconds', t('workspace-total-time-limit-seconds')], ['max_agents', t('settings-maximum-agents')], ['max_concurrent_agents', t('settings-concurrent-agents')], ['max_depth', t('settings-maximum-collaboration-depth')]];
  return <Modal title={t('workspace-resume', { v0: fileName(run.target_path) })} description={t('workspace-continue-from-saved-progress-budgets-are-cumulative')} onClose={onClose}>
    <div className="settings-grid">{fields.map(([field, label]) => <label className="field" key={field}><span>{label}</span><input aria-label={label} type="number" min={1} value={Number(budget[field])} onChange={event => setBudget(value => ({ ...value, [field]: Number(event.target.value) }))} /></label>)}</div>
    <label className="field"><span>{t('workspace-total-cost-limit-usd-blank-for-unlimited')}</span><input aria-label={t('workspace-total-cost-limit')} type="number" min={0.01} step="any" value={budget.max_usd ?? ''} onChange={event => setBudget(value => ({ ...value, max_usd: event.target.value === '' ? null : Number(event.target.value) }))} /></label>
    {error && <p className="error" role="alert">{errorText(error)}</p>}
    <div className="modal-footer"><button onClick={onClose}>{t('workspace-cancel')}</button><button className="primary" disabled={pending || fields.some(([field]) => !Number.isSafeInteger(budget[field]) || Number(budget[field]) <= 0) || (budget.max_usd != null && (!Number.isFinite(budget.max_usd) || budget.max_usd <= 0))} onClick={() => {
      setPending(true); void onResume(budget).catch(error => setError(errorValue(error))).finally(() => setPending(false));
    }}><Play size={14} />{pending ? t('workspace-resuming') : t('workspace-resume-analysis')}</button></div>
  </Modal>;
}
