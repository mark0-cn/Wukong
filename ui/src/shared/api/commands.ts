import type { Budget, ConfigDocument, ConfigEdit, Envelope, EvidenceView, FindingDetails, Json, ToolRecord } from './types';
import type { Preferences, PreferenceSnapshot, LocalizedMessage } from '../i18n';

export type UpdateAction = 'status' | 'health' | 'doctor' | 'start_tools' | 'stop_tools' | 'select_tools' | 'check' | 'prepare' | 'setup' | 'install' | 'recover' | 'integrate' | 'clean_cache' | 'clean_images';
export interface EnvironmentHealth {
  platform?: 'linux' | 'windows' | null;
  epoch?: number | null;
  configured: boolean; docker: boolean | null; running: boolean | null; tools: boolean | null;
  checked_at: string | null; tools_checked_at: string | null; image: string | null;
  checks: Record<string, boolean>; error: LocalizedMessage | null;
}
export interface UpdateStatus {
  health?: EnvironmentHealth;
  diagnosis?: 'daemon' | 'credentials' | 'network' | 'rate_limit' | 'disk' | 'unknown' | null;
  log_available?: boolean;
  cancellable?: boolean; cancelling?: boolean;

  operation_id?: string | null; revision?: number; busy?: boolean; action?: UpdateAction | null;
  version: string; phase: string; message: string | LocalizedMessage; available: string | null; notes: string;
  downloaded: number; total: number | null; prepared: string | null; recovery: string | null;
  needs_setup: boolean; pending: boolean; environment_step?: string | null;
  image_version?: string | null; image_progress?: ImagePullProgress | null;
}
export interface ImagePullProgress {
  layers: { id: string; state: 'waiting' | 'downloading' | 'downloaded' | 'extracting' | 'verifying' | 'ready' }[];
  elapsed_ms: number; quiet_ms?: number; finished: boolean; cached: boolean; truncated: boolean;
}
export type WorkbenchRequest =
  | { action: 'status' | 'up' | 'down' | 'doctor' | 'list' }
  | { action: 'import'; source: string; id: string; binary: string | null }
  | { action: 'prepare' | 'build' | 'start' | 'stop'; id: string }
  | { action: 'select'; id: string | null }
  | { action: 'export'; destination: string };
type Epoch = { epoch: number };
type Run = Epoch & { run: string };
type Item = Run & { id: string };
type Command<A, R> = { args: A; result: R };
export interface WindowsConnectionInput {
  connection_json: string; name: string; host: string; identity_file: string;
  confirmed_fingerprint: string; make_default: boolean;
}
export type WorkerMediaState = 'generated' | 'installed_pending' | 'available';
export type WorkerMediaRecordState = 'present' | 'missing' | 'corrupt' | 'unknown';
export type WorkerMediaVerificationState = 'not_started' | 'incomplete' | 'verified' | 'unknown';
export interface WorkerMediaInstallationDiagnostics {
  generation_record: WorkerMediaRecordState; recovery_record: WorkerMediaRecordState;
  receipt: WorkerMediaRecordState; registration: WorkerMediaRecordState;
  verification: WorkerMediaVerificationState; detail?: string;
}
export interface WorkerMediaDiagnostics {
  inventory: WorkerMediaRecordState;
  unreadable_generation_records: { path: string; installation_id?: string; error: string }[];
  detail?: string;
}
export interface WorkerMediaInstallation {
  installation_id: string; environment_id: string; environment_name: string;
  state: WorkerMediaState; app_version: string; ssh_port: number;
  management_peer: string; network_prefix_len: number;
  zip_path: string; zip_present: boolean; media_path: string; media_present: boolean;
  created_at: string; registered_environment_id: string | null;
  registered_environment_name: string | null; last_address: string | null;
  candidate_addresses: string[]; warning: string | null;
  diagnostics: WorkerMediaInstallationDiagnostics;
}
export interface WorkerMediaEnvironment {
  environment_id: string; name: string; platform: string; installation_id: string | null;
  ssh_host: string | null; managed_ssh: boolean; worker_media_state: WorkerMediaState | null;
}
export interface WorkerMediaStatus {
  environments: WorkerMediaEnvironment[];
  installations: WorkerMediaInstallation[];
  diagnostics: WorkerMediaDiagnostics;
}
export interface WorkerMediaNetwork {
  id: string; name: string; description: string; ipv4: string; prefix_len: number;
  rfc1918: boolean; virtual_adapter: boolean; recommended: boolean;
}
export interface WorkerMediaTemplate {
  path: string; source: 'local' | 'downloaded' | 'cached';
  version: string | null; sha256: string | null;
}
export interface WorkerMediaInstructions {
  summary: string; setup_executable: string; receipt_filename: string; steps: string[];
}
export interface WorkerMediaGeneration {
  installation_id: string; environment_id: string; environment_name: string;
  installation_state: WorkerMediaState; app_version: string;
  zip_path: string; zip_sha256: string; zip_size: number; zip_entries: number;
  media_path: string; registry_path: string; ssh_port: number;
  management_peer: string; network_prefix_len: number; network: WorkerMediaNetwork;
  template: WorkerMediaTemplate; instructions: WorkerMediaInstructions;
}
export interface WorkerMediaImportEnvironment {
  environment_id: string; name: string; ssh_host: string | null;
}
export interface WorkerMediaImportResult {
  installation_id: string; environment_id: string; state: WorkerMediaState;
  environment: WorkerMediaImportEnvironment | null; address: string | null;
  candidate_count: number; guidance: string | null;
}

/** Native command boundary: result types follow command names, never callers. */
export interface Commands {
  cleanup_preview: Command<{ selection: { data: boolean; tools: boolean; challenges: boolean } }, CleanupPreview>;
  cleanup_confirm: Command<{ fingerprint: string }, void>;
  cleanup_cancel: Command<undefined, void>;
  preferences_read: Command<undefined, PreferenceSnapshot>;
  preferences_save: Command<{ preferences: Preferences }, PreferenceSnapshot>;
  workspace_read: Command<{ request: { run: string | null; agent: string | null; event_limit: number; known_revision: string | null } }, Envelope>;
  start_run: Command<Epoch & { target: string; task: string }, string>;
  resume_run: Command<Run & { budget: Budget }, string>;
  pause_run: Command<Run, void>;
  stop_agent: Command<Run & { agent: string }, void>;
  send_message: Command<Run & { agent: string; generation: number; text: string; followUp: boolean }, Json>;
  pick_target: Command<undefined, string | null>;
  pick_directory: Command<undefined, string | null>;
  export_report: Command<Run, string | null>;
  config_read: Command<Epoch, ConfigDocument>;
  config_open: Command<Epoch, ConfigDocument | null>;
  config_preview: Command<{ document: ConfigDocument; edits: ConfigEdit[] }, ConfigDocument>;
  windows_connection_preview: Command<{ document: ConfigDocument; edits: ConfigEdit[]; input: WindowsConnectionInput }, ConfigDocument>;
  pick_ssh_identity: Command<undefined, string | null>;
  worker_media_status: Command<Epoch, WorkerMediaStatus>;
  worker_media_networks: Command<undefined, WorkerMediaNetwork[]>;
  worker_media_generate: Command<{ epoch: number; template?: string; output: string; environmentName?: string; network?: string }, WorkerMediaGeneration>;
  worker_media_import: Command<{ epoch: number; receipt: string; confirmedFingerprint: string; name?: string }, WorkerMediaImportResult>;
  worker_media_retry: Command<{ epoch: number; installationId: string }, WorkerMediaImportResult>;
  pick_worker_template: Command<undefined, string | null>;
  pick_worker_output: Command<undefined, string | null>;
  pick_worker_receipt: Command<undefined, string | null>;
  config_save: Command<Epoch & { document: ConfigDocument; edits: ConfigEdit[] }, ConfigDocument>;
  finding_details: Command<Item, FindingDetails>;
  evidence_preview: Command<Item & { offset: number }, EvidenceView>;
  tool_details: Command<Item, ToolRecord>;
  shutdown_workspace: Command<undefined, void>;
  update_action: Command<Epoch & { action: UpdateAction }, UpdateStatus>;
  update_log_directory: Command<{ operationId: string }, void>;
  update_diagnostics: Command<{ operationId: string }, string | null>;
  update_cancel: Command<{ operationId: string }, UpdateStatus>;
  update_storage: Command<Epoch, { data_dir: string; cache_dir: string }>;
  update_cache_directory: Command<Epoch & { destination: string }, string>;
  update_data_directory: Command<Epoch & { destination: string }, void>;
  workbench_action: Command<Epoch & { request: WorkbenchRequest }, Json>;
  observability_status: Command<Epoch, ObservabilityStatus>;
  observability_run: Command<Epoch & { request: ObservabilityRunRequest }, ObservabilityRunSnapshot>;
  observability_local_activity: Command<Epoch & { run: string; cursor: string | null; limit: number }, ObservabilityLocalActivity>;
  observability_traces: Command<Epoch & { run: string; request: ObservabilityPageRequest }, ObservabilitySignal<ObservabilityTraceList>>;
  observability_trace: Command<Epoch & { run: string; traceId: string }, ObservabilitySignal<ObservabilityTrace>>;
  observability_logs: Command<Epoch & { run: string; traceId: string | null; request: ObservabilityPageRequest }, ObservabilitySignal<ObservabilityLogBatch>>;
  observability_metrics: Command<Epoch & { run: string; metric: ObservabilityMetricName; range: ObservabilityTimeRange; limit: number }, ObservabilitySignal<ObservabilityMetricBatch>>;
  observability_grafana_link: Command<Epoch & { target: ObservabilityGrafanaTarget; range: ObservabilityTimeRange }, string | null>;
}
export interface CleanupPreview {
  fingerprint: string; files: number; bytes: number; paths: string[];
  resources: {kind: string; id: string; name: string}[];
  retained: {path: string; reason: string}[]; errors: string[]; truncated: boolean;
}

export interface ObservabilityTimeRange { start: string; end: string }
export interface ObservabilityPageRequest { range: ObservabilityTimeRange; limit: number; cursor: string | null }
export interface ObservabilityBackendError { kind: string; message: string; status?: number }
export interface ObservabilitySignal<T> {
  configured: boolean; partial: boolean; truncated: boolean; stale: boolean; queried_at: string;
  error: ObservabilityBackendError | null; data: T | null;
}
export interface ObservabilityBackendCapability { configured: boolean; origin: string | null; credential_available: boolean }
export interface ObservabilityDeliveryHealth { healthy: boolean | null; last_error: string | null }
export interface ObservabilityTelemetryStatus {
  activated: boolean; traces: boolean; metrics: boolean; logs: boolean; local_jsonl: boolean; worker_relay: boolean;
  worker_relay_status: { traces: ObservabilityDeliveryHealth; logs: ObservabilityDeliveryHealth };
  last_error: string | null;
}
export interface ObservabilityStatus {
  capabilities: { tempo: ObservabilityBackendCapability; loki: ObservabilityBackendCapability; prometheus: ObservabilityBackendCapability; grafana: ObservabilityBackendCapability };
  exporter: ObservabilityTelemetryStatus;
  queried_at: string;
}
export interface ObservabilityTraceSummary {
  trace_id: string; root_service_name: string | null; root_trace_name: string | null;
  start_time: string | null; duration_ms: number; span_count: number;
}
export interface ObservabilityTraceList { run_id: string; traces: ObservabilityTraceSummary[]; next_cursor: string | null }
export interface ObservabilityTraceSpan {
  trace_id: string; span_id: string; parent_span_id: string | null; name: string;
  start_time: string | null; end_time: string | null; duration_nanos: number | null; status: string | null;
  attributes: Record<string, Json>; resource: Record<string, Json>;
}
export interface ObservabilityTrace { trace_id: string; spans: ObservabilityTraceSpan[] }
export interface ObservabilityLogEntry {
  timestamp: string; timestamp_nanos: string; line: string; line_truncated: boolean;
  labels: { service_name: string | null; level: string | null; target: string | null };
  attributes: Record<string, Json>;
}
export interface ObservabilityLogBatch { run_id: string; trace_id: string | null; entries: ObservabilityLogEntry[]; next_cursor: string | null }
export type ObservabilityMetricName = 'runs_started' | 'runs_finished' | 'active_runs' | 'run_duration_count' |
  'agent_generation_duration_count' | 'worker_rpc_duration_count' | 'worker_rpc_retries' | 'model_operation_duration_count' |
  'model_token_usage_count' | 'evidence_sync_duration_count' | 'telemetry_dropped';
export interface ObservabilityMetricBatch {
  metric: ObservabilityMetricName; expression: string; scope: 'service'; step_seconds: number;
  series: { labels: Record<string, string>; points: { timestamp: string; value: number }[] }[];
}
export interface ObservabilityLocalRun {
  id: string; status: string; root_id: string; created_at: string; updated_at: string;
  total_tokens: number; cost_usd: number; summary: string | null;
}
export interface ObservabilityLocalAgent {
  id: string; parent_id: string | null; name: string; role: string; status: string; generation: number; turns: number;
  total_tokens: number; cost_usd: number; created_at: string; updated_at: string;
}
export interface ObservabilityLocalEvent { seq: number; agent_id: string | null; kind: string; created_at: string; data: Json; data_truncated: boolean }
export interface ObservabilityToolCounts { total: number; prepared: number; running: number; completed: number; failed: number; uncertain: number; side_effecting: number }
export interface ObservabilityResourceCounts { jobs: number; active_jobs: number; terminal_jobs: number; evidence_pending: number }
export interface ObservabilityLocalActivity {
  activity_kind: 'local_business_activity'; run: ObservabilityLocalRun; agents: ObservabilityLocalAgent[]; events: ObservabilityLocalEvent[];
  tools: ObservabilityToolCounts; resources: ObservabilityResourceCounts; next_cursor: string | null;
  has_more: boolean; partial: boolean; truncated: boolean; stale: boolean; queried_at: string;
}
export interface ObservabilityRunRequest {
  run: string; range: ObservabilityTimeRange; limit: number; trace_cursor: string | null; log_cursor: string | null;
  local_cursor: string | null; metric: ObservabilityMetricName;
}
export interface ObservabilityRunSnapshot {
  local: ObservabilityLocalActivity; traces: ObservabilitySignal<ObservabilityTraceList>;
  logs: ObservabilitySignal<ObservabilityLogBatch>; metrics: ObservabilitySignal<ObservabilityMetricBatch>;
}
export type ObservabilityGrafanaTarget = { traces: { run: string } } | { logs: { run: string; trace_id: string | null } } | { metrics: { metric: ObservabilityMetricName } };
export interface Events {
  'workspace-changed': unknown;
  'close-requested': { busy: boolean };
}
