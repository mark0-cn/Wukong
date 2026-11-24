import { useRef } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { t } from '../../shared/i18n';
import type { ObservabilityLocalActivity as LocalActivityData } from '../../shared/api/commands';
import { Status, compact } from '../../shared/ui/Status';
import { formatNumber, formatTime, jsonText, LocalNotices } from './Status';

export function LocalActivity({ activity, onLoadMore, loadingMore }: {
  activity: LocalActivityData; onLoadMore: () => void; loadingMore: boolean;
}) {
  const eventsRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: activity.events.length, getScrollElement: () => eventsRef.current, estimateSize: () => 72,
    getItemKey: index => activity.events[index]?.seq ?? index, overscan: 8,
  });
  const { run, tools, resources } = activity;
  return <div className="observability-local">
    <section className="observability-section" aria-labelledby="observability-local-run">
      <div className="observability-section-heading"><h3 id="observability-local-run">{t('observability-local-run')}</h3><Status value={run.status} /><LocalNotices partial={activity.partial} truncated={activity.truncated} stale={activity.stale} /></div>
      <div className="observability-kpis">
        <div><span>{t('observability-total-tokens')}</span><strong>{compact(run.total_tokens)}</strong></div>
        <div><span>{t('observability-cost')}</span><strong>${run.cost_usd.toFixed(4)}</strong></div>
        <div><span>{t('observability-agents')}</span><strong>{formatNumber(activity.agents.length)}</strong></div>
        <div><span>{t('observability-tools-total')}</span><strong>{formatNumber(tools.total)}</strong></div>
        <div><span>{t('observability-jobs-active')}</span><strong>{formatNumber(resources.active_jobs)}</strong></div>
        <div><span>{t('observability-evidence-pending')}</span><strong>{formatNumber(resources.evidence_pending)}</strong></div>
      </div>
      <p className="muted observability-run-identity">{run.id} · {t('observability-updated')} {formatTime(run.updated_at)}</p>
      {run.summary && <p>{run.summary}</p>}
    </section>
    <section className="observability-section">
      <h3>{t('observability-tool-job-state')}</h3>
      <div className="observability-count-grid">
        {([
          ['prepared', tools.prepared], ['running', tools.running], ['completed', tools.completed], ['failed', tools.failed],
          ['uncertain', tools.uncertain], ['side-effecting', tools.side_effecting],
        ] as const).map(([label, value]) => <div key={label}><span>{t(`observability-tools-${label}`)}</span><strong>{formatNumber(value)}</strong></div>)}
        <div><span>{t('observability-jobs-total')}</span><strong>{formatNumber(resources.jobs)}</strong></div>
        <div><span>{t('observability-jobs-terminal')}</span><strong>{formatNumber(resources.terminal_jobs)}</strong></div>
      </div>
    </section>
    <section className="observability-section">
      <h3>{t('observability-agents')}</h3>
      <div className="observability-table-scroll"><table className="observability-table"><thead><tr><th>{t('observability-agent')}</th><th>{t('observability-role')}</th><th>{t('observability-state')}</th><th>{t('observability-turns')}</th><th>{t('observability-total-tokens')}</th></tr></thead>
        <tbody>{activity.agents.map(agent => <tr key={agent.id}><td><strong>{agent.name}</strong><small>{agent.id}</small></td><td>{agent.role}</td><td><Status value={agent.status} /></td><td>{agent.turns}</td><td>{formatNumber(agent.total_tokens)}</td></tr>)}</tbody></table></div>
      {!activity.agents.length && <p className="muted">{t('observability-no-agents')}</p>}
    </section>
    <section className="observability-section observability-events-section">
      <div className="observability-section-heading"><h3>{t('observability-local-events')}</h3><span className="muted">{formatNumber(activity.events.length)}</span></div>
      <div className="observability-virtual-list" ref={eventsRef} role="list" tabIndex={0} aria-label={t('observability-local-events')}>
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map(item => {
            const event = activity.events[item.index];
            return <article key={item.key} data-index={item.index} ref={virtualizer.measureElement} className="observability-event" role="listitem"
              style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}>
              <header><span className="observability-seq">#{event.seq}</span><strong>{event.kind}</strong><time>{formatTime(event.created_at)}</time></header>
              {event.agent_id && <span className="muted">{event.agent_id}</span>}
              <pre>{jsonText(event.data)}</pre>{event.data_truncated && <span className="observability-flag truncated">{t('observability-truncated')}</span>}
            </article>;
          })}
        </div>
      </div>
      {!activity.events.length && <p className="muted">{t('observability-no-local-events')}</p>}
      {activity.next_cursor && <button disabled={loadingMore} onClick={onLoadMore}>{loadingMore ? t('observability-loading') : t('observability-load-newer-events')}</button>}
    </section>
  </div>;
}
