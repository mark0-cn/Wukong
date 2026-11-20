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
}
export interface CleanupPreview {
  fingerprint: string; files: number; bytes: number; paths: string[];
  resources: {kind: string; id: string; name: string}[];
  retained: {path: string; reason: string}[]; errors: string[]; truncated: boolean;
}
export interface Events {
  'workspace-changed': unknown;
  'close-requested': { busy: boolean };
}
