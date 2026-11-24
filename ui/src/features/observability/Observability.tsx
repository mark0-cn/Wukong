import { useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { t } from '../../shared/i18n';
import type { ObservabilityMetricName } from '../../shared/api/commands';
import { useObservability } from './model';
import { BackendStatus } from './Status';
import { LocalActivity } from './LocalActivity';
import { Traces } from './Traces';
import { Logs } from './Logs';
import { Metrics } from './Metrics';

export type ObservabilityView = 'local' | 'traces' | 'logs' | 'metrics';
const views: ObservabilityView[] = ['local', 'traces', 'logs', 'metrics'];

export function Observability({ epoch, run, runStatus }: { epoch: number; run: string; runStatus: string }) {
  const [view, setView] = useState<ObservabilityView>('local');
  const [rangeMinutes, setRangeMinutes] = useState(60);
  const [metric, setMetric] = useState<ObservabilityMetricName>('runs_started');
  const [logTrace, setLogTrace] = useState('');
  const [logTraceDraft, setLogTraceDraft] = useState('');
  const model = useObservability({ epoch, run, runStatus, rangeMinutes, metric, logTrace });
  const grafanaDisabled = Boolean(model.status && !model.status.capabilities.grafana.configured) || model.linkBusy;
  return <div className="observability-workspace">
    <header className="observability-header">
      <div><h2>{t('observability-title')}</h2><p>{t('observability-description')}</p></div>
      <div className="observability-header-actions"><label><span>{t('observability-time-range')}</span><select value={rangeMinutes} onChange={event => setRangeMinutes(Number(event.target.value))}>
        {([15, 60, 360, 1440] as const).map(value => <option key={value} value={value}>{t(`observability-range-${value}`)}</option>)}
      </select></label><button className="icon-button" title={t('observability-refresh')} aria-label={t('observability-refresh')} disabled={model.loading} onClick={model.reload}><RefreshCw size={15} className={model.loading ? 'spin' : ''} /></button>
      </div>
    </header>
    <BackendStatus status={model.status} error={model.statusError} />
    <nav className="observability-navigation" aria-label={t('observability-views')}>
      {views.map(value => <button key={value} className={view === value ? 'active' : ''} aria-pressed={view === value} onClick={() => setView(value)}>{t(`observability-view-${value}`)}</button>)}
    </nav>
    {model.error && <div className="observability-global-error" role="alert">{model.error}{model.data && <span>{t('observability-retained-local-data')}</span>}</div>}
    {model.linkMessage && <p className={model.linkMessage === 'unconfigured' ? 'muted' : 'error'} role="status">{model.linkMessage === 'unconfigured' ? t('observability-grafana-unconfigured') : model.linkMessage}</p>}
    {model.pageError && <p className="error" role="alert">{model.pageError}</p>}
    {!model.data ? <p className="muted observability-empty">{model.loading ? t('observability-loading') : t('observability-no-data')}</p> :
      <div className={`observability-data ${model.loading ? 'refreshing' : ''}`}>
        {view === 'local' && <LocalActivity activity={model.data.local} loadingMore={model.pageBusy === 'local'} onLoadMore={() => { void model.loadMoreLocal(); }} />}
        {view === 'traces' && <Traces signal={model.data.traces} epoch={epoch} run={run} loadingMore={model.pageBusy === 'traces'} onLoadMore={() => { void model.loadMoreTraces(); }} onGrafana={model.openGrafana} grafanaDisabled={grafanaDisabled} />}
        {view === 'logs' && <Logs signal={model.data.logs} run={run} filter={logTrace} filterDraft={logTraceDraft} onFilterDraft={setLogTraceDraft}
          onApplyFilter={() => setLogTrace(logTraceDraft.trim())} onClearFilter={() => { setLogTrace(''); setLogTraceDraft(''); }}
          loadingMore={model.pageBusy === 'logs'} onLoadMore={() => { void model.loadMoreLogs(); }} onGrafana={model.openGrafana} grafanaDisabled={grafanaDisabled} />}
        {view === 'metrics' && <Metrics signal={model.data.metrics} metric={metric} onMetric={setMetric} onGrafana={model.openGrafana} grafanaDisabled={grafanaDisabled} />}
      </div>}
  </div>;
}
