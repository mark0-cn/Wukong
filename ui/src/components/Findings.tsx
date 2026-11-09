import { useEffect, useState } from 'react';
import { ChevronLeft, ChevronRight, FileSearch, ShieldCheck } from 'lucide-react';
import { api, errorText } from '../bridge';
import type { Evidence, EvidenceView, Finding, FindingDetails } from '../types';
import { Markdown } from './Markdown';
import { Status } from './Status';

export function EvidenceReader({ evidence, epoch, run }: { evidence: Evidence; epoch: number; run: string }) {
  const [offset, setOffset] = useState(0);
  const [preview, setPreview] = useState<EvidenceView>();
  const [error, setError] = useState('');
  useEffect(() => {
    let alive = true;
    setPreview(undefined); setError('');
    void api<EvidenceView>('evidence_preview', { epoch, run, id: evidence.id, offset }).then(value => { if (alive) setPreview(value); })
      .catch(error => { if (alive) setError(errorText(error)); });
    return () => { alive = false; };
  }, [epoch, run, evidence.id, offset]);
  return <section className="evidence-reader"><div className="evidence-heading"><strong>{evidence.source}</strong><span>{evidence.media_type} · {evidence.size.toLocaleString()} B</span></div>
    <div className="evidence-paging"><button aria-label="上一页证据" disabled={offset === 0} onClick={() => setOffset(value => Math.max(0, value - 16_384))}><ChevronLeft size={14} /></button>
      <span>{preview ? `${preview.offset.toLocaleString()}–${preview.end.toLocaleString()} / ${preview.size.toLocaleString()} B${preview.binary ? ' · 十六进制' : ''}` : '正在读取证据…'}</span>
      <button aria-label="下一页证据" disabled={!preview || preview.end >= preview.size} onClick={() => { if (preview) setOffset(preview.end); }}><ChevronRight size={14} /></button></div>
    {error && <p className="error" role="alert">{error}</p>}{preview && <pre>{preview.text || '（空文件）'}</pre>}
    <div className="evidence-hash">SHA-256 {evidence.sha256}</div>
  </section>;
}
function FindingInspector({ epoch, run, finding, onAgent }: { epoch: number; run: string; finding: Finding; onAgent: (id: string) => void }) {
  const [details, setDetails] = useState<FindingDetails>();
  const [error, setError] = useState('');
  const [evidence, setEvidence] = useState<string>();
  useEffect(() => {
    let alive = true;
    setDetails(undefined); setError(''); setEvidence(undefined);
    void api<FindingDetails>('finding_details', { epoch, run, id: finding.id }).then(value => { if (alive) setDetails(value); })
      .catch(error => { if (alive) setError(errorText(error)); });
    return () => { alive = false; };
  }, [epoch, run, finding.id, finding.updated_at]);
  if (error) return <p className="error" role="alert">{error}</p>;
  if (!details) return <p className="muted">正在读取问题与验证记录…</p>;
  const selected = details.evidence.find(value => value.id === evidence);
  return <article className="finding-inspector"><div className="finding-tags"><Status value={finding.stage} />{finding.severity && <span className={`severity severity-${finding.severity}`}>{finding.severity}</span>}</div>
    <h2>{finding.title}</h2><Markdown text={finding.description} />
    {finding.root_cause && <><h3>根因</h3><Markdown text={finding.root_cause} /></>}
    {finding.impact && <><h3>影响</h3><Markdown text={finding.impact} /></>}
    {finding.severity_rationale && <p className="muted">评级依据：{finding.severity_rationale}</p>}
    <button className="subtle" onClick={() => onAgent(finding.agent_id)}>查看发现此问题的 Agent</button>
    <h3><ShieldCheck size={16} />验证结果</h3>
    {details.verification ? <div className="verification"><Status value={details.verification.passed ? 'verified' : 'failed'} />
      <span>成功 {details.verification.successes} / {details.verification.attempts} 次</span><Markdown text={details.verification.impact} />
      <details><summary>环境、条件与原始结果</summary><pre>{JSON.stringify({ environment: details.verification.environment, conditions: details.verification.conditions, result: details.verification.result }, null, 2)}</pre></details>
    </div> : <p className="muted">尚无验证记录。</p>}
    <h3><FileSearch size={16} />证据与来源</h3>
    {details.missing_evidence.length > 0 && <p className="error">部分证据缺失：{details.missing_evidence.join(', ')}</p>}
    {!details.evidence.length && <p className="muted">暂无关联证据。</p>}
    <div className="evidence-tabs">{details.evidence.map(item => <button key={item.id} title={item.id} className={evidence === item.id ? 'selected' : ''} onClick={() => setEvidence(item.id)}>{item.source}</button>)}</div>
    {selected && <EvidenceReader key={`${epoch}:${run}:${selected.id}`} evidence={selected} epoch={epoch} run={run} />}
    {details.calls.length > 0 && <details className="source-calls"><summary>{details.calls.length} 项来源操作</summary>{details.calls.map(call => <div key={call.id}><strong>{call.name}</strong><Status value={call.state} /><pre>{JSON.stringify(call.arguments, null, 2)}</pre></div>)}</details>}
  </article>;
}
export function Findings({ findings, epoch, run, onAgent }: { findings: Finding[]; epoch: number; run: string; onAgent: (id: string) => void }) {
  const [selected, setSelected] = useState<string>();
  const [filter, setFilter] = useState('');
  const [stage, setStage] = useState('');
  const visible = findings.filter(finding => (!stage || finding.stage === stage) && `${finding.title} ${finding.description} ${finding.severity ?? ''}`.toLowerCase().includes(filter.toLowerCase()));
  const finding = visible.find(finding => finding.id === selected) ?? visible[0];
  return <div className="findings-page"><div className="finding-list"><input aria-label="搜索问题" placeholder="搜索问题…" value={filter} onChange={event => setFilter(event.target.value)} />
    <select aria-label="按验证阶段筛选" value={stage} onChange={event => setStage(event.target.value)}><option value="">所有阶段</option>{[...new Set(findings.map(finding => finding.stage))].map(stage => <option key={stage} value={stage}>{stage}</option>)}</select>
    {visible.map(item => <button key={item.id} className={`finding-card ${finding?.id === item.id ? 'selected' : ''}`} onClick={() => setSelected(item.id)}>
      <strong>{item.title}</strong><div><Status value={item.stage} />{item.severity && <span className={`severity severity-${item.severity}`}>{item.severity}</span>}</div></button>)}
    {!visible.length && <p className="muted">{findings.length ? '没有匹配的问题。' : '发现的问题及验证结果将在这里展示。'}</p>}</div>
    <div className="finding-detail-scroll">{finding ? <FindingInspector key={`${epoch}:${run}:${finding.id}`} epoch={epoch} run={run} finding={finding} onAgent={onAgent} /> : <div className="empty-state"><ShieldCheck size={28} /><h3>用证据支撑结论</h3><p>每个发现都可追溯到工具操作、原始证据和验证记录。</p></div>}</div>
  </div>;
}
