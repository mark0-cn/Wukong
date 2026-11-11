import { t, text, useLocale } from '../i18n';
import type { Host, Run, Workspace } from '../types';
import { compact, labels, StateIcon } from './Status';
import { ToolDuration } from './ToolActivity';

export function WorkspaceStatus({ run, data }: { run: Run; data: Workspace }) {
  useLocale();
  const active = data.active_tools[0];
  const error = ['failed', 'crashed'].includes(run.status);
  const hint = run.status === 'completed' ? t('ui-completed-hint')
    : run.status === 'stopped' ? t('ui-stopped-hint')
    : error ? t('ui-failed-hint')
    : run.status === 'budget_paused' ? t('workspace-progress-saved-ready-to-resume')
    : run.task;
  return <div data-status={run.status} className={`work-status ${error ? 'work-error' : ''}`}>
    <span className="work-status-icon"><StateIcon state={active?.state ?? run.status} /></span>
    <div><strong>{active ? text(active.title) : labels[run.status]}</strong>
      <p title={active ? active.target || active.purpose : hint}>{active ? `${active.target || active.purpose}${data.active_tools.length > 1 ? t('workspace-more-operations', { v0: data.active_tools.length - 1 }) : ''}` : hint}</p></div>
    {active && <ToolDuration display={active} />}
  </div>;
}

export function WorkspaceFooter({ host, run, error, agentCount }: { host?: Host; run?: Run; error: boolean; agentCount: number }) {
  useLocale();
  const connection = error ? 'error' : host ? 'ready' : 'connecting';
  return <footer className="workspace-footer">
    <span><span className={`connection-dot connection-${connection}`} />{error ? t('workspace-connection-error') : host ? t('workspace-local-engine') : t('workspace-connecting-to-local-engine')}</span>
    {host && <details className="usage-menu"><summary>{t('ui-model-and-usage')}</summary><div>
      <strong>{host.model}</strong>
      {run && <><p>{compact(run.total_tokens)} / {compact(run.budget.max_total_tokens)} tokens</p><p>{t('workspace-total-cost')}: ${run.cost_usd.toFixed(4)}</p><p>{t('workspace-agent-count', { count: agentCount })}</p></>}
      <p>{host.execution ? t('workspace-analysis-running-in-background') : t('workspace-all-progress-is-saved-locally')}</p>
    </div></details>}
  </footer>;
}
