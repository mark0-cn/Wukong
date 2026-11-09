import type { Agent, Envelope, Finding, Host, Run, ToolDisplay, Workspace } from '../types';

export const budget = { max_agents: 24, max_depth: 4, max_concurrent_agents: 4, max_turns_per_agent: 40,
  max_total_tokens: 500000, max_usd: null, max_wall_seconds: 3600, role_token_shares: {} };
export const host: Host = { epoch: 1, initial_run: null, config_path: '/tmp/re-agent.toml', state_dir: '/tmp/re-agent-state',
  model: 'gpt-5.2', budget, execution: { operation: 'execution-1', run_id: 'run-a', phase: 'running' }, pending_operations: 0, notice: null };
export const runs: Run[] = [{ id: 'run-a', target_path: '/samples/parser.elf', task: '分析输入解析逻辑，调查长度检查并独立验证可疑问题。',
  status: 'running', root_id: 'root', budget, created_at: '2026-11-09T03:10:00Z', total_tokens: 42800, cost_usd: 0.1482, summary: null },
{ id: 'run-b', target_path: '/samples/archive.elf', task: '还原归档文件的解包流程。', status: 'budget_paused', root_id: 'root-b', budget,
  created_at: '2026-11-08T08:10:00Z', total_tokens: 500000, cost_usd: 1.81, summary: '已定位主要解包逻辑，等待继续验证边界条件。' }];
export const agents: Agent[] = [
  { id: 'root', name: '主分析 Agent', role: 'root', parent_id: null, scope: '全局协调', generation: 0, task: runs[0].task, status: 'waiting', depth: 0, turns: 6, total_tokens: 14000, summary: null, error: null },
  { id: 'analyst', name: '输入边界分析', role: 'analyst', parent_id: 'root', scope: 'parse_record', generation: 0, task: '跟踪 parse_record 的长度计算与内存写入，定位边界检查。', status: 'running', depth: 1, turns: 8, total_tokens: 19000, summary: null, error: null },
  { id: 'verifier', name: '复现与独立验证', role: 'verifier', parent_id: 'root', scope: '受控输入复现', generation: 1, task: '构造最小输入，独立验证越界写入的触发条件。', status: 'running', depth: 1, turns: 4, total_tokens: 9800, summary: null, error: null },
];
export const tool: ToolDisplay = { call_id: 'call-info', title: '查看文件信息', target: 'parser.elf', purpose: '确认架构和入口点，选择后续分析方法。',
  state: 'completed', summary: 'ELF · x86_64 · 已识别入口点 0x401090', duration_ms: 184, evidence_id: 'evidence-info' };
export const failedTool: ToolDisplay = { ...tool, call_id: 'call-debug', title: '启动调试会话', target: 'parser.elf', purpose: '独立复核输入触发的行为。', state: 'failed', summary: '调试后端暂不可用：连接已关闭。已保留当前证据。', duration_ms: 1200, evidence_id: null };
export const finding: Finding = { id: 'finding-a', run_id: 'run-a', agent_id: 'analyst', title: '记录长度未覆盖后续拷贝范围',
  description: '在 `parse_record` 中，输入长度的校验与后续拷贝使用了不同的边界。', root_cause: '长度计算截断后，拷贝仍使用原始长度。',
  impact: '特定输入可导致越界写入；影响仍需独立复核。', severity: 'high', severity_rationale: '输入可控，已获得受控复现。',
  stage: 'reproduced', evidence_ids: ['evidence-a'], verification_id: null, updated_at: '2026-11-09T03:15:00Z' };
export const workspace: Workspace = { selected: 'run-a', runs, agents, findings: [finding], notes: [{ id: 'note-a', agent_id: 'analyst', title: '解析逻辑观察', body: '长度字段为小端序，拷贝目标为栈缓冲区。', kind: 'analysis' }], active_tools: [{ ...tool, call_id: 'call-xrefs', title: '查找交叉引用', target: 'parse_record', state: 'running', summary: '', duration_ms: null }], has_earlier: false,
  transcript: [
    { id: 'intro', role: 'assistant', author: '主分析 Agent', agent_id: 'root', created_at: '2026-11-09T03:10:01Z', content: [{ type: 'text', text: '我会先确认文件结构，再把输入解析和独立验证拆成两个任务。\n\n重点关注 **长度计算、边界检查和内存拷贝**。每项结论都会关联证据。' }, { type: 'tool-call', toolCallId: tool.call_id, toolName: 'binary_info', args: { display: tool }, result: { display: tool }, isError: false }] },
    { id: 'progress', role: 'assistant', author: '输入边界分析', agent_id: 'analyst', created_at: '2026-11-09T03:11:10Z', content: [{ type: 'text', text: '已定位到 `parse_record`。长度检查与后续拷贝使用的数值不一致，接下来沿调用点核对输入是否可控。\n\n```c\nuint16_t size = input_length + 8;\nmemcpy(buffer, input, input_length);\n```' }] },
    { id: 'operator', role: 'user', author: '你 → 主分析 Agent', agent_id: 'root', created_at: '2026-11-09T03:12:00Z', delivery: 'included', content: [{ type: 'text', text: '优先验证真实可触发的输入，保留最小复现样例。' }] },
    { id: 'retry', role: 'assistant', author: '复现与独立验证', agent_id: 'verifier', created_at: '2026-11-09T03:13:00Z', content: [{ type: 'text', text: '调试连接中断，当前操作未完成。我会记录失败原因，待后端恢复后继续复核。' }, { type: 'tool-call', toolCallId: failedTool.call_id, toolName: 'debug_start', args: { display: failedTool }, result: { display: failedTool }, isError: true }] },
  ] };
export const envelope = (data: Workspace = workspace): Envelope => ({ host, workspace: { revision: 'revision-a', data } });
