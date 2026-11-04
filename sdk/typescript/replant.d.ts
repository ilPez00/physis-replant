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

export interface Receipt {
  id: string;
  key: string;
  operation: 'record' | 'replay' | 'lookup' | 'approve' | 'reject' | 'undo';
  status: 'started' | 'succeeded' | 'failed' | 'miss' | 'blocked' | 'review' | 'rejected';
  engine_version: string;
  strategy: string;
  policy_hash: string;
  recipe_hash?: string | null;
  provider_calls: number;
  elapsed_us: number;
  calls: Array<Record<string, unknown>>;
  candidates?: ReviewCandidate[];
  reason?: string | null;
}

export interface Stats {
  executions: number;
  verified_records: number;
  verified_replays: number;
  failures: number;
  misses: number;
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
  inspect(request: ReplantRequest): Promise<unknown>;
  stats(): Promise<Stats>;
  approve(request: ReplantRequest, candidate: string): Promise<Receipt>;
  reject(request: ReplantRequest, candidate: string): Promise<Receipt>;
  undo(id: string): Promise<Receipt>;
}
