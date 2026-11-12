// Compile-only public contract assertions; no server or effects.
import { ReplantClient, type Receipt, type Recipe, type ToolSpec, type Stats } from './replant';

const legacy: ToolSpec = { kind: 'action', program: '/operator/bin/apply' };
const multiple: ToolSpec = { kinds: ['action', 'undo'], program: '/operator/bin/rollback', timeout_ms: 1_800_000 };
// @ts-expect-error exactly one role declaration form is allowed
const ambiguous: ToolSpec = { kind: 'action', kinds: ['undo'], program: '/operator/bin/apply' };
// @ts-expect-error no implicit action permission
const missing: ToolSpec = { program: '/operator/bin/apply' };
const bound: Recipe = { bind_inputs: true, steps: [{ tool: 'apply', input: { days: { $input: 'days' } } }], checks: [{ tool: 'verify' }] };

function contract(client: ReplantClient, receipt: Receipt, stats: Stats) {
  const rerecord: (id: string) => Promise<Receipt> = client.rerecord.bind(client);
  const staleCount: number = stats.stale;
  const stderr: string | undefined = receipt.calls[0]?.stderr;
  const exitCode: number | null | undefined = receipt.calls[0]?.exit_code;
  if (receipt.status === 'stale' && receipt.stale_record) {
    const id: string = receipt.stale_record.id;
    const template: Recipe = receipt.stale_record.recipe;
    return { id, template, rerecord, staleCount, stderr, exitCode };
  }
  return { rerecord, staleCount, stderr, exitCode };
}

void [legacy, multiple, ambiguous, missing, bound, contract];
