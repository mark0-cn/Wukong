import { useEffect, useState } from 'react';
import { message, t, useLocale } from '../../shared/i18n';
import type { ToolDisplay } from '../../shared/api/types';

/** Worker timestamps/counts only; elapsed time is not a completion percentage. */
export function ToolDuration({ display }: { display: ToolDisplay }) {
  const [now, setNow] = useState(Date.now);
  const running = display.job && ['running', 'queued'].includes(display.state);
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);
  const elapsed = display.duration_ms ?? (running ? Math.max(0, now - Date.parse(display.job!.started_at)) : null);
  return elapsed == null ? null : <span className="tool-duration">{(elapsed / 1000).toFixed(1)}s</span>;
}

/** A finished execution and resource cleanup are independent facts. */
export function JobFacts({ display }: { display: ToolDisplay }) {
  useLocale();
  const job = display.job;
  if (!job) return null;
  const execution = job.execution.finished
    ? job.execution.outcome === 'unknown' ? t('progress-job-execution-unknown') : t('progress-job-execution-finished')
    : t('progress-job-execution-running');
  const counts = [job.attempts != null && t('progress-job-attempts', { count: job.attempts }),
    job.successes != null && t('progress-job-successes', { count: job.successes })].filter(Boolean);
  return <div className="job-facts">
    <p className="tool-summary job-fact job-execution">
      <span className="job-fact-label">{t('progress-job-execution')}</span> {execution}
      {job.execution.exit_code != null && <> · <code>exit {job.execution.exit_code}</code></>}
    </p>
    <p className="tool-summary job-fact job-cleanup">
      <span className="job-fact-label">{t('progress-job-cleanup')}</span> {t(`progress-job-cleanup-${job.cleanup.status}`)}
      {job.cleanup.evidence_id && <> · <code>{job.cleanup.evidence_id}</code></>}
    </p>
    {job.blocking && <p className="tool-summary job-fact job-blocking" role="status">
      <span className="job-fact-label">{t('progress-job-blocking')}</span> {message(job.blocking.reason)}
      {job.blocking.detail && <code title={job.blocking.detail}>{job.blocking.detail}</code>}
    </p>}
    {counts.length > 0 && <p className="tool-summary muted">{counts.join(' · ')}</p>}
    {job.evidence.length > 0 && <p className="tool-summary muted job-evidence">
      <span className="job-fact-label">{t('progress-job-evidence')}</span>
      {job.evidence.map(link => <span key={`${link.role}-${link.evidence_id}`} className="job-evidence-link">
        {t(`progress-job-evidence-${link.role}`)} <code>{link.evidence_id}</code>
      </span>)}
    </p>}
  </div>;
}
