export interface ReplantRequest {
  prompt: string;
  input?: Record<string, unknown>;
  context?: Record<string, unknown>;
}

export interface ToolCall {
  tool: string;
  input?: unknown;
}

export interface Recipe {
  bind_inputs?: boolean;
  preconditions?: ToolCall[];
  steps: ToolCall[];
  checks: ToolCall[];
  undo?: ToolCall[];
}

export interface ReplantRecord {
  request: ReplantRequest;
  recipe: Recipe;
  trace: Record<string, unknown>;
}

export interface ReviewCandidate {
  id: string;
  similarity: number;
  request: ReplantRequest;
}

export type ToolKind = 'action' | 'check' | 'undo';

export type ToolSpec = {
  program: string;
  args?: string[];
  fingerprint_files?: string[];
  timeout_ms?: number;
} & ({ kind: ToolKind; kinds?: never } | { kind?: never; kinds: ToolKind[] });

export interface ToolOutcome {
  tool: string;
  kind: ToolKind;
  phase: string;
  ok: boolean;
  exit_code: number | null;
  output: unknown | null;
  error: string | null;
  elapsed_us: number;
  stdout?: string;
  stderr?: string;
  stdout_truncated?: boolean;
  stderr_truncated?: boolean;
}

export interface StaleRecord {
  id: string;
  recipe: Recipe;
}

export interface Receipt {
  id: string;
  key: string;
  operation: 'record' | 'rerecord' | 'replay' | 'lookup' | 'approve' | 'reject' | 'undo';
  status: 'started' | 'succeeded' | 'failed' | 'miss' | 'stale' | 'blocked' | 'review' | 'rejected';
  engine_version: string;
  strategy: string;
  policy_hash: string;
  recipe_hash?: string | null;
  provider_calls: number;
  elapsed_us: number;
  calls: ToolOutcome[];
  candidates?: ReviewCandidate[];
  stale_record?: StaleRecord | null;
  reason?: string | null;
}

export interface Stats {
  executions: number;
  verified_records: number;
  verified_replays: number;
  failures: number;
  misses: number;
  stale: number;
  reviews: number;
  rejected: number;
  undos: number;
  undo_failures: number;
  blocked: number;
  pending: number;
  replay_share_percent: number;
}

export class ReplantError extends Error {
  status?: number;
  receipt: unknown;
}

export class ReplantClient {
  constructor(baseUrl: string, token: string, options?: { fetchImpl?: typeof fetch; timeoutMs?: number });
  health(): Promise<{ status: string }>;
  run(request: ReplantRequest): Promise<Receipt>;
  record(record: ReplantRecord): Promise<Receipt>;
  rerecord(id: string): Promise<Receipt>;
  inspect(request: ReplantRequest): Promise<unknown>;
  stats(): Promise<Stats>;
  approve(request: ReplantRequest, candidate: string): Promise<Receipt>;
  reject(request: ReplantRequest, candidate: string): Promise<Receipt>;
  undo(id: string): Promise<Receipt>;
}
