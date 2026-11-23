import { useRef } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { t } from '../../shared/i18n';
import type { ObservabilityLogBatch, ObservabilitySignal } from '../../shared/api/commands';
import { formatTime, GrafanaButton, jsonText, SignalNotices } from './Status';
import type { ObservabilityGrafanaTarget } from '../../shared/api/commands';

export function Logs({ signal, run, filter, filterDraft, onFilterDraft, onApplyFilter, onClearFilter, onLoadMore, loadingMore, onGrafana, grafanaDisabled }: {
  signal?: ObservabilitySignal<ObservabilityLogBatch>; run: string; filter: string; filterDraft: string;
  onFilterDraft: (value: string) => void; onApplyFilter: () => void; onClearFilter: () => void;
  onLoadMore: () => void; loadingMore: boolean; onGrafana: (target: ObservabilityGrafanaTarget) => Promise<void>; grafanaDisabled: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const entries = signal?.data?.entries ?? [];
  const virtualizer = useVirtualizer({
    count: entries.length, getScrollElement: () => scrollRef.current, estimateSize: () => 82,
    getItemKey: index => `${entries[index]?.timestamp_nanos ?? index}:${index}`, overscan: 10,
  });
  return <section className="observability-signal-view" aria-labelledby="observability-logs-heading">
    <div className="observability-view-heading"><div><h3 id="observability-logs-heading">{t('observability-logs')}</h3><p className="muted">{t('observability-logs-description')}</p></div>
      <GrafanaButton target={{ logs: { run, trace_id: filter || null } }} onOpen={onGrafana} disabled={grafanaDisabled} /></div>
    <form className="observability-filter" onSubmit={event => { event.preventDefault(); onApplyFilter(); }}>
      <label><span>{t('observability-trace-id-filter')}</span><input value={filterDraft} onChange={event => onFilterDraft(event.target.value)} placeholder={t('observability-all-traces')} spellCheck={false} /></label>
      <button type="submit">{t('observability-apply-filter')}</button>
      {filter && <button type="button" className="subtle" onClick={onClearFilter}>{t('observability-clear-filter')}</button>}
    </form>
    <SignalNotices signal={signal} backend="loki" />
    {signal?.data && <>
      <div className="observability-virtual-list observability-log-list" ref={scrollRef} role="list" tabIndex={0} aria-label={t('observability-log-entries')}>
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map(item => {
            const entry = entries[item.index];
            return <article key={item.key} data-index={item.index} ref={virtualizer.measureElement} className="observability-log-entry" role="listitem"
              style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}>
              <header><time>{formatTime(entry.timestamp)}</time>{entry.labels.level && <span className={`observability-level level-${entry.labels.level.toLowerCase()}`}>{entry.labels.level}</span>}
                {entry.labels.service_name && <span>{entry.labels.service_name}</span>}{entry.labels.target && <code>{entry.labels.target}</code>}</header>
              <pre>{entry.line}</pre>
              {entry.line_truncated && <span className="observability-flag truncated">{t('observability-line-truncated')}</span>}
              {Object.keys(entry.attributes).length > 0 && <details><summary>{t('observability-attributes')}</summary><dl className="observability-properties">
                {Object.entries(entry.attributes).map(([key, value]) => <div key={key}><dt>{key}</dt><dd><pre>{jsonText(value)}</pre></dd></div>)}
              </dl></details>}
            </article>;
          })}
        </div>
      </div>
      {!entries.length && <p className="muted">{t('observability-no-logs')}</p>}
      {signal.data.next_cursor && <button disabled={loadingMore} onClick={onLoadMore}>{loadingMore ? t('observability-loading') : t('observability-load-more')}</button>}
    </>}
  </section>;
}
