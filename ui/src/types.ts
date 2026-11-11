import type { LocalizedMessage } from './i18n';
export type Status = 'queued' | 'running' | 'waiting' | 'budget_paused' | 'completed' | 'stopped' | 'crashed' | 'failed';
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export interface Budget {
  max_agents: number; max_depth: number; max_concurrent_agents: number;
  max_turns_per_agent: number; max_total_tokens: number; max_usd: number | null;
  max_wall_seconds: number; role_token_shares: Record<string, number>;
}
export interface Run {
  id: string; target_path: string; task: string; status: Status; root_id: string;
  budget: Budget; created_at: string; total_tokens: number; cost_usd: number; summary: string | null;
}
export interface Agent {
  id: string; name: string; role: string; parent_id: string | null; scope: string;
  generation: number; task: string; status: Status; depth: number; turns: number;
  total_tokens: number; summary: string | null; error: string | null;
}
export interface ToolDisplay {
  call_id: string; title: string; target: string; purpose: string; state: string;
  summary: string | LocalizedMessage; duration_ms: number | null; evidence_id: string | null;
  preview?: { language: string; text: string; truncated: boolean; evidence_id: string };
  job?: { id: string; state: string; stale: boolean; started_at: string; finished_at: string | null;
    observed_at: string; attempts: number | null; successes: number | null };
}
export interface ChatMessage {
  id: string; role: 'assistant' | 'user'; agent_id: string | null; author: string | LocalizedMessage;
  created_at: string; delivery?: string;
  content: ({ type: 'text'; text: string | LocalizedMessage } | { type: 'tool-call'; toolCallId: string;
    toolName: string; args: { display: ToolDisplay }; result?: { display: ToolDisplay }; isError: boolean })[];
}
export interface Finding {
  id: string; run_id: string; agent_id: string; title: string; description: string;
  root_cause: string | null; impact: string | null; severity: string | null;
  severity_rationale: string | null; stage: string; evidence_ids: string[]; verification_id: string | null; updated_at: string;
}
export interface Note { id: string; agent_id: string; title: string; body: string; kind: string }
export interface Workspace {
  selected: string | null; runs: Run[]; agents: Agent[]; findings: Finding[]; notes: Note[];
  transcript: ChatMessage[]; active_tools: ToolDisplay[]; has_earlier: boolean;
}
export interface Host {
  epoch: number; initial_run: string | null; config_path: string; state_dir: string;
  model: string; budget: Budget; execution: { operation: string; run_id: string | null; phase: string } | null;
  pending_operations: number; notice: { id: number; error: boolean; message: string | LocalizedMessage } | null;
}
export interface Envelope { host: Host; workspace: { revision: string; data: Workspace | null } }
export interface ConfigDocument { path: string; source: string; original: string | null; form: Record<string, Json> }
export interface ConfigEdit { path: string[]; value: Json }
export interface Evidence {
  id: string; agent_id: string; tool_call_id: string; source: string; media_type: string; sha256: string; size: number;
}
export interface EvidenceView extends Evidence { offset: number; end: number; text: string; binary: boolean }
export interface FindingDetails {
  finding: Finding; verification: { passed: boolean; attempts: number; successes: number; impact: string;
    environment: Json; conditions: Json; result: Json } | null;
  evidence: Evidence[]; calls: ToolRecord[]; missing_evidence: string[];
}
export interface ToolRecord {
  id: string; agent_id: string; name: string; arguments: Json; state: string;
  result: Json; evidence_id: string | null; created_at: string; updated_at: string;
}
