import { Check, LoaderCircle, TriangleAlert } from 'lucide-react';
import { t } from '../i18n';

export function SetupProgress({ step, busy, failed }: { step?: string | null; busy: boolean; failed: boolean }) {
  const current = step === 'docker' ? 0 : step === 'pulling' ? 1 : step === 'validating' || step === 'activating' ? 2 : -1;
  const steps = [
    [t('updates-step-docker'), t('updates-step-docker-help')],
    [t('updates-step-image'), t('updates-step-image-help')],
    [t('updates-step-validation'), t('updates-step-validation-help')],
  ];
  return <ol className={`setup-steps ${busy ? 'setup-steps-compact' : ''}`} aria-label={t('updates-setup-progress')}>
    {steps.map(([title, description], index) => {
      const active = index === current, done = current > index;
      const state = done ? 'done' : active && failed ? 'failed' : active && busy ? 'active' : 'pending';
      const label = done ? t('updates-step-done') : active && failed ? t('updates-setup-failed') : active && busy ? t('updates-setup-working') : t('updates-step-pending');
      return <li key={index} data-state={state} aria-current={active ? 'step' : undefined}>
        <span className="setup-step-icon" aria-hidden="true">{done ? <Check size={18} /> : active && failed ? <TriangleAlert size={18} /> : active && busy ? <LoaderCircle size={18} /> : index + 1}</span>
        <div><div className="setup-step-title"><strong>{title}</strong><span>{label}</span></div>{busy && !active ? <details><summary>{t('updates-step-details')}</summary><p>{description}</p></details> : <p>{description}</p>}</div>
      </li>;
    })}
  </ol>;
}
