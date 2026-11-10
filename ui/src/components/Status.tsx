import { t, useLocale, currentLocale } from '../i18n';
import { Check, Circle, CircleAlert, LoaderCircle, Pause, Square } from 'lucide-react';

export const labels: Record<string, string> = new Proxy({} as Record<string, string>, { get: (_, key: string) => ({
  queued: t('workspace-queued'), prepared: t('workspace-queued'), running: t('workspace-running'), waiting: t('workspace-waiting-for-collaborators-2'),
  budget_paused: t('workspace-paused'), paused: t('workspace-paused'), completed: t('workspace-completed'), stopped: t('workspace-stopped'),
  crashed: t('workspace-crashed'), failed: t('workspace-failed'), uncertain: t('workspace-result-uncertain'), interrupted: t('workspace-interrupted'),
  hypothesis: t('workspace-needs-verification'), reported: t('workspace-reported'), verified: t('workspace-verified'), rejected: t('workspace-rejected'),
  candidate: t('workspace-candidate-finding'), queued_delivery: t('workspace-awaiting-delivery'), included: t('workspace-delivered'), replied: t('workspace-replied'), stale: t('workspace-expired'),
  suspected: t('workspace-needs-verification'), reproduced: t('workspace-reproduced'), root_cause_confirmed: t('workspace-root-cause-confirmed'), impact_verified: t('workspace-impact-verified'), inconclusive: t('workspace-inconclusive'),
  root: t('workspace-lead-agent'), analyst: t('workspace-analysis-agent'), verifier: t('workspace-verification-agent'),
} as Record<string, string>)[key] });
export function StateIcon({ state }: { state: string }) {
  useLocale();
  if (state === 'running') return <LoaderCircle size={14} className="spin" />;
  if (state === 'completed' || state === 'verified') return <Check size={14} />;
  if (['failed', 'crashed', 'uncertain', 'interrupted'].includes(state)) return <CircleAlert size={14} />;
  if (state === 'paused' || state === 'budget_paused') return <Pause size={13} />;
  if (state === 'stopped') return <Square size={12} />;
  return <Circle size={12} />;
}
export function Status({ value }: { value: string }) {
  useLocale();
  return <span className={`status status-${value}`}><StateIcon state={value} />{labels[value] ?? value}</span>;
}
export const terminal = (status: string) => ['completed', 'stopped', 'crashed', 'failed'].includes(status);
export const compact = (value: number) => new Intl.NumberFormat(currentLocale(), { notation: 'compact', maximumFractionDigits: 1 }).format(value);
export const fileName = (value: string) => value.split(/[\\/]/).filter(Boolean).at(-1) ?? value;
