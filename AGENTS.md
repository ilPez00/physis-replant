# physis-replant — agent notes

<!-- BEGIN free-agent-orchestration -->
## Free-agent orchestration (fleet convention)

Proceed with free-agent orchestration for development work whenever possible.

- Route tasks through the fleet orchestrator instead of paid APIs:
  `free-agent run "<task>" -w <repo> --json` — it picks the free worker with
  live quota headroom and fails over automatically.
- Bounded build/verify loops:
  `free-agent loop "<task>" -w <repo> --check "<cmd>" --max-iterations N`.
- Planners/orchestrators: `--planner bl` (Bailian qwen4-max) or `--planner bl-flash`
  (qwen-flash-character, verified free); other planners: codex, claude, grok, kimi, agy.
- Check pool state first: `free-agent status`. Workers include freebuff/free-pi
  (daily free, top priority), opencode, copilot, pi-<provider> variants (keys
  resolved from ~/.env), bl/bl-flash (Bailian free grants), commandcode,
  omniroute (127.0.0.1:20128), fcm (:19280), freeglm (:20530) when their
  daemons are up.
- Free keys live in ~/.env (never commit them). Add new free routes data-driven
  (agents.json / `free-agent cli-agents add`), not by hardcoding in Python.
- Keep credentials out of task text and logs. Free tiers may retain prompts:
  never paste secrets; be deliberate with sensitive code.
- Paid/subscription routes only when the operator explicitly asks.
<!-- END free-agent-orchestration -->

## Commit + push to air's USB storage — always

Work in this repo is not saved until it is in **three** places, not two: a
commit on the local branch, that commit on `origin` (GitHub `ilPez00`), **and a
`git bundle` on air's USB stick** (`/dev/sdb1`, 29.8 G exfat, mounted on air at
`/mnt/sdb1`). The stick is the off-machine copy that survives a dead node and a
dead NAS; `origin` alone is not enough.

`fleet-sync` does this — run it at the end of any session that touched files,
and after any multi-file edit wave:

```bash
fleet-sync            # commit + push + bundle to the NAS and to air's stick
```

When the stick is mounted on air, the bundle is pushed there by the same run
(tier `air-usb`). When it is not mounted, that tier is **skipped with a log
line — never faked, never filled onto `/` instead**. Do not substitute a tarball
of the working tree: the portable, restorable, verifiable artifact is
`git bundle create <repo>.bundle --all`, and `git clone <repo>.bundle` must
restore it.

Details and the rest of the storage tiers: `~/.config/agents/SYNC.md`.
