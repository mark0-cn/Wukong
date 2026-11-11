import { t, text, useLocale, currentLocale } from '../i18n';
import { useEffect, useState } from 'react';
import { ChevronLeft, ChevronRight, FileSearch, ShieldCheck } from 'lucide-react';
import { api, errorText, errorValue } from '../bridge';
import type { Evidence, EvidenceView, Finding, FindingDetails, Json } from '../types';
import { CopyButton } from './CopyButton';
import { Markdown } from './Markdown';
import { labels, Status } from './Status';

function EnvironmentFacts({ value }: { value: Json }) {
  useLocale();
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const fields = Object.entries(value).filter(([, item]) => item !== null && typeof item !== 'object').slice(0, 8);
  if (!fields.length) return null;
  return <details className="environment-facts"><summary>{t('ui-environment-facts')}</summary><dl>{fields.map(([key, item]) => <div key={key}><dt>{key}</dt><dd>{String(item)}</dd></div>)}</dl></details>;
}

export function EvidenceReader({ evidence, epoch, run }: { evidence: Evidence; epoch: number; run: string }) {
  useLocale();
  const [offset, setOffset] = useState(0);
  const [preview, setPreview] = useState<EvidenceView>();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  useEffect(() => {
    let alive = true;
    setPreview(undefined); setError('');
    void api('evidence_preview', { epoch, run, id: evidence.id, offset }).then(value => { if (alive) setPreview(value); })
      .catch(error => { if (alive) setError(errorValue(error)); });
    return () => { alive = false; };
  }, [epoch, run, evidence.id, offset]);
  return <section className="evidence-reader"><div className="evidence-heading"><strong>{evidence.source}</strong><CopyButton value={evidence.source} label={t('ui-copy-source')} /><span>{evidence.media_type} · {evidence.size.toLocaleString(currentLocale())} B</span></div>
    <div className="evidence-paging"><button aria-label={t('workspace-previous-evidence-page')} disabled={offset === 0} onClick={() => setOffset(value => Math.max(0, value - 16_384))}><ChevronLeft size={14} /></button>
      <span>{preview ? `${preview.offset.toLocaleString(currentLocale())}–${preview.end.toLocaleString(currentLocale())} / ${preview.size.toLocaleString(currentLocale())} B${preview.binary ? t('workspace-hexadecimal') : ''}` : t('workspace-loading-evidence')}</span>
      <button aria-label={t('workspace-next-evidence-page')} disabled={!preview || preview.end >= preview.size} onClick={() => { if (preview) setOffset(preview.end); }}><ChevronRight size={14} /></button></div>
    {error && <p className="error" role="alert">{errorText(error)}</p>}{preview && <pre>{preview.text || t('workspace-empty-file')}</pre>}
    <div className="evidence-hash"><span>SHA-256 <code>{evidence.sha256}</code></span><CopyButton value={evidence.sha256} label={t('ui-copy-hash')} /><span>ID <code>{evidence.id}</code></span><CopyButton value={evidence.id} label={t('ui-copy-evidence-id')} /></div>
  </section>;
}
function FindingInspector({ epoch, run, finding, onAgent }: { epoch: number; run: string; finding: Finding; onAgent: (id: string) => void }) {
  useLocale();
  const [details, setDetails] = useState<FindingDetails>();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [evidence, setEvidence] = useState<string>();
  useEffect(() => {
    let alive = true;
    setDetails(undefined); setError(''); setEvidence(undefined);
    void api('finding_details', { epoch, run, id: finding.id }).then(value => { if (alive) setDetails(value); })
      .catch(error => { if (alive) setError(errorValue(error)); });
    return () => { alive = false; };
  }, [epoch, run, finding.id, finding.updated_at]);
  if (error) return <p className="error" role="alert">{errorText(error)}</p>;
  if (!details) return <p className="muted">{t('workspace-loading-finding-and-verification')}</p>;
  const selected = details.evidence.find(value => value.id === evidence) ?? (details.evidence.length === 1 ? details.evidence[0] : undefined);
  return <article className="finding-inspector"><div className="finding-tags"><Status value={finding.stage} />{finding.severity && <span className={`severity severity-${finding.severity}`}>{text(`severity-${finding.severity}`, {}, finding.severity)}</span>}</div>
    <h2>{finding.title}</h2>
    <h3><ShieldCheck size={16} />{t('workspace-verification-results')}</h3>
    {details.verification ? <div className="verification"><Status value={details.verification.passed ? 'verified' : 'failed'} />
      <span>{t('workspace-verification-count', { successes: details.verification.successes, attempts: details.verification.attempts })}</span><Markdown text={details.verification.impact} />
      <EnvironmentFacts value={details.verification.environment} /><details><summary>{t('workspace-environment-conditions-and-raw-results')}</summary><pre>{JSON.stringify({ environment: details.verification.environment, conditions: details.verification.conditions, result: details.verification.result }, null, 2)}</pre></details>
    </div> : <p className="muted">{t('workspace-no-verification-record-yet')}</p>}
    {details.missing_evidence.length > 0 && <p className="error">{t('workspace-missing-evidence')}{details.missing_evidence.join(', ')}</p>}
    <Markdown text={finding.description} />
    {finding.root_cause && <><h3>{t('workspace-root-cause')}</h3><Markdown text={finding.root_cause} /></>}
    {finding.impact && <><h3>{t('workspace-impact')}</h3><Markdown text={finding.impact} /></>}
    {finding.severity_rationale && <p className="muted">{t('workspace-rating-rationale')}{finding.severity_rationale}</p>}
    <button className="subtle" onClick={() => onAgent(finding.agent_id)}>{t('workspace-view-the-agent-that-found-this-issue')}</button>
    <h3><FileSearch size={16} />{t('workspace-evidence-and-sources')}</h3>
    {!details.evidence.length && <p className="muted">{t('workspace-no-linked-evidence-yet')}</p>}
    {details.evidence.length > 1 && <div className="evidence-tabs" role="group" aria-label={t('ui-select-evidence')}>{details.evidence.map(item => <button key={item.id} title={item.id} className={evidence === item.id ? 'selected' : ''} onClick={() => setEvidence(item.id)}>{item.source}</button>)}</div>}
    {selected && <EvidenceReader key={`${epoch}:${run}:${selected.id}`} evidence={selected} epoch={epoch} run={run} />}
    {details.calls.length > 0 && <details className="source-calls"><summary>{t('workspace-source-operation-count', { count: details.calls.length })}</summary>{details.calls.map(call => <div key={call.id}><strong>{call.name}</strong><Status value={call.state} /><pre>{JSON.stringify(call.arguments, null, 2)}</pre></div>)}</details>}
  </article>;
}
export function Findings({ findings, epoch, run, onAgent }: { findings: Finding[]; epoch: number; run: string; onAgent: (id: string) => void }) {
  useLocale();
  const [selected, setSelected] = useState<string>();
  const [filter, setFilter] = useState('');
  const [stage, setStage] = useState('');
  const visible = findings.filter(finding => (!stage || finding.stage === stage) && `${finding.title} ${finding.description} ${finding.severity ?? ''}`.toLowerCase().includes(filter.toLowerCase()));
  const finding = visible.find(finding => finding.id === selected) ?? visible[0];
  return <div className="findings-page" data-detail={Boolean(selected && visible.some(item => item.id === selected))}><div className="finding-list"><input aria-label={t('workspace-search-findings')} placeholder={t('workspace-search-findings-2')} value={filter} onChange={event => setFilter(event.target.value)} />
    <select aria-label={t('workspace-filter-by-verification-stage')} value={stage} onChange={event => setStage(event.target.value)}><option value="">{t('workspace-all-stages')}</option>{[...new Set(findings.map(finding => finding.stage))].map(stage => <option key={stage} value={stage}>{labels[stage] ?? stage}</option>)}</select>
    {visible.map(item => <button key={item.id} className={`finding-card ${finding?.id === item.id ? 'selected' : ''}`} onClick={() => setSelected(item.id)}>
      <strong>{item.title}</strong><div><Status value={item.stage} />{item.severity && <span className={`severity severity-${item.severity}`}>{text(`severity-${item.severity}`, {}, item.severity)}</span>}</div></button>)}
    {!visible.length && <p className="muted">{findings.length ? t('workspace-no-matching-findings') : t('workspace-findings-and-verification-results-will-appear-here')}</p>}</div>
    <div className="finding-detail-scroll"><button className="findings-back subtle" onClick={() => setSelected(undefined)}><ChevronLeft size={14} />{t('ui-back-findings')}</button>{finding ? <FindingInspector key={`${epoch}:${run}:${finding.id}`} epoch={epoch} run={run} finding={finding} onAgent={onAgent} /> : <div className="empty-state"><ShieldCheck size={28} /><h3>{t('workspace-support-conclusions-with-evidence')}</h3><p>{t('workspace-every-finding-links-back-to-tool-operations')}</p></div>}</div>
  </div>;
}
