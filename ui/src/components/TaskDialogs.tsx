import { useState } from 'react';
import { FolderOpen, Play } from 'lucide-react';
import { api, errorText } from '../bridge';
import type { Budget, Run } from '../types';
import { Modal } from './Modal';
import { fileName } from './Status';

export const defaultTask = '对目标二进制开展逆向分析，识别并验证可利用的安全问题，提供可复核的证据与结论。';
export function NewTask({ target, task, setTarget, setTask, onClose, onStart, busy }: {
  target: string; task: string; setTarget: (value: string) => void; setTask: (value: string) => void;
  onClose: () => void; onStart: () => Promise<void>; busy: boolean;
}) {
  const [error, setError] = useState('');
  const [pending, setPending] = useState(false);
  const pick = async () => {
    try { const path = await api('pick_target'); if (path) setTarget(path); }
    catch (error) { setError(errorText(error)); }
  };
  return <Modal title="开始新的分析" description="选择本地目标并描述你希望 Agent 完成的工作。" onClose={onClose}>
    <label className="field"><span>目标文件</span><div className="input-action"><input autoFocus aria-label="目标文件路径" value={target} placeholder="选择或拖入一个二进制文件" onChange={event => setTarget(event.target.value)} /><button onClick={() => { void pick(); }} aria-label="选择目标文件"><FolderOpen size={17} /></button></div></label>
    <label className="field"><span>分析任务</span><textarea aria-label="分析任务" rows={6} value={task} onChange={event => setTask(event.target.value)} /></label>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="modal-footer"><span className="muted">文件与证据保存在当前工作区</span><button onClick={onClose}>取消</button>
      <button className="primary" disabled={!target.trim() || !task.trim() || pending || busy} onClick={() => {
        setPending(true); setError(''); void onStart().catch(error => setError(errorText(error))).finally(() => setPending(false));
      }}><Play size={14} />{pending ? '正在准备…' : '开始分析'}</button></div>
    {busy && <p className="muted">当前工作尚未保存完成，请先暂停正在运行的分析。</p>}
  </Modal>;
}
export function ResumeTask({ run, onClose, onResume }: { run: Run; onClose: () => void; onResume: (budget: Budget) => Promise<void> }) {
  const [budget, setBudget] = useState<Budget>({ ...run.budget });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const fields: [keyof Budget, string][] = [['max_total_tokens', '总 tokens 上限'], ['max_turns_per_agent', '每个 Agent 轮数'], ['max_wall_seconds', '总时限（秒）'], ['max_agents', '最大 Agent 数'], ['max_concurrent_agents', '同时运行 Agent 数'], ['max_depth', '最大协作层级']];
  return <Modal title={`恢复 ${fileName(run.target_path)}`} description="从已保存的进度继续。预算为任务累计上限，可提高以继续被预算暂停的任务。" onClose={onClose}>
    <div className="settings-grid">{fields.map(([field, label]) => <label className="field" key={field}><span>{label}</span><input aria-label={label} type="number" min={1} value={Number(budget[field])} onChange={event => setBudget(value => ({ ...value, [field]: Number(event.target.value) }))} /></label>)}</div>
    <label className="field"><span>累计费用上限（USD，留空表示不限制）</span><input aria-label="累计费用上限" type="number" min={0.01} step="any" value={budget.max_usd ?? ''} onChange={event => setBudget(value => ({ ...value, max_usd: event.target.value === '' ? null : Number(event.target.value) }))} /></label>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="modal-footer"><button onClick={onClose}>取消</button><button className="primary" disabled={pending || fields.some(([field]) => !Number.isSafeInteger(budget[field]) || Number(budget[field]) <= 0) || (budget.max_usd != null && (!Number.isFinite(budget.max_usd) || budget.max_usd <= 0))} onClick={() => {
      setPending(true); void onResume(budget).catch(error => setError(errorText(error))).finally(() => setPending(false));
    }}><Play size={14} />{pending ? '正在恢复…' : '恢复分析'}</button></div>
  </Modal>;
}
