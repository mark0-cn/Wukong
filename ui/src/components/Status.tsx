import { Check, Circle, CircleAlert, LoaderCircle, Pause, Square } from 'lucide-react';

export const labels: Record<string, string> = {
  queued: '等待执行', prepared: '等待执行', running: '进行中', waiting: '等待协作',
  budget_paused: '已暂停', paused: '已暂停', completed: '已完成', stopped: '已停止',
  crashed: '异常退出', failed: '失败', uncertain: '结果待确认', interrupted: '已中断',
  hypothesis: '待验证', reported: '已报告', verified: '已验证', rejected: '已排除',
  candidate: '候选问题', queued_delivery: '待接收', included: '已接收', replied: '已回复', stale: '已过期',
  suspected: '待验证', reproduced: '已复现', root_cause_confirmed: '根因已确认', impact_verified: '影响已验证', inconclusive: '尚无定论',
  root: '主 Agent', analyst: '分析 Agent', verifier: '验证 Agent',
};
export function StateIcon({ state }: { state: string }) {
  if (state === 'running') return <LoaderCircle size={14} className="spin" />;
  if (state === 'completed' || state === 'verified') return <Check size={14} />;
  if (['failed', 'crashed', 'uncertain', 'interrupted'].includes(state)) return <CircleAlert size={14} />;
  if (state === 'paused' || state === 'budget_paused') return <Pause size={13} />;
  if (state === 'stopped') return <Square size={12} />;
  return <Circle size={12} />;
}
export function Status({ value }: { value: string }) {
  return <span className={`status status-${value}`}><StateIcon state={value} />{labels[value] ?? value}</span>;
}
export const terminal = (status: string) => ['completed', 'stopped', 'crashed', 'failed'].includes(status);
export const compact = (value: number) => new Intl.NumberFormat('zh-CN', { notation: 'compact', maximumFractionDigits: 1 }).format(value);
export const fileName = (value: string) => value.split(/[\\/]/).filter(Boolean).at(-1) ?? value;
