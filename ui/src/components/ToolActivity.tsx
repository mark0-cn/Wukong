import { useEffect, useState } from 'react';
import { t, useLocale } from '../i18n';
import type { ToolDisplay } from '../types';

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

export function JobFacts({ display }: { display: ToolDisplay }) {
  useLocale();
  const job = display.job;
  if (!job || !job.stale && job.attempts == null && job.successes == null) return null;
  return <p className="tool-summary muted">
    {[job.stale && t('progress-job-stale'), job.attempts != null && t('progress-job-attempts', { count: job.attempts }),
      job.successes != null && t('progress-job-successes', { count: job.successes })].filter(Boolean).join(' · ')}
  </p>;
}
