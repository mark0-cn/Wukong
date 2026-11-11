import type { Budget, ConfigDocument, ConfigEdit, Envelope, EvidenceView, FindingDetails, Json, ToolRecord } from './types';
import type { Preferences, PreferenceSnapshot, LocalizedMessage } from './i18n';

export type UpdateAction = 'status' | 'health' | 'doctor' | 'start_tools' | 'stop_tools' | 'select_tools' | 'check' | 'prepare' | 'setup' | 'install' | 'recover' | 'integrate' | 'clean_cache' | 'clean_images';
export interface EnvironmentHealth {
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

/** Native command boundary: result types follow command names, never callers. */
export interface Commands {
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
export interface Events {
  'workspace-changed': unknown;
  'close-requested': { busy: boolean };
}
