# Documentation

Start with the [README](../README.md) for installation and a first run.

| Document | Contents |
| --- | --- |
| [Architecture](architecture.md) | Layers, modules, data flow, and design principles |
| [Monitoring and skills](monitoring.md) | Run journals, debugging, local diagnostics, and built-in guidance |
| [Configuration](configuration.md) | Commands, flags, limits, exit codes, and report behavior |
| [Context management](context-management.md) | Tool-output projection, recent-history protection, temporary recovery, and implementation references |
| [Binary releases](releases.md) | AMD64 GNU, musl, and Windows downloads, checksums, and release CI |
| [Authentication](authentication.md) | API-key and Sign In With ChatGPT (Codex) credentials |
| [Protocol](protocol.md) | Version 1 agent request/response and dataset contract |
| [Evaluation](evaluation.md) | Matching, scoring, metrics, and reproducibility |
| [Constraints](constraints.md) | Required harness invariants, existing checks, and enforcement gaps |
| [Harness design research](harness-design.md) | Primary-source findings, project applications, and optimization acceptance criteria |
| [Validation](validation.md) | Recorded live rounds, checks, and per-round results |
| [Experiment log](experiments.md) | Round ledger, accumulated experience, and future directions |
| [Reports](reports/deepseek-evaluation.json) | Committed evaluation artifact |
| [Tiny baseline round 1](reports/tiny-baseline-round1.json) | Blind live evaluation over the seven submodule fixtures |
| [Tiny baseline round 2](reports/tiny-baseline-round2.json) | Same blind run after the JSON-mode and reasoning-effort fixes |
| [Tiny baseline round 3](reports/tiny-baseline-round3.json) | Same blind run after the configurable stream cap |
| [Tiny paired round 4](reports/tiny-round4.json) | Old vs threshold prompt after the limit-forwarding fix |
| [Tiny paired round 5](reports/tiny-round5.json) | Same comparison after the forced final-report fix |
| [Tiny strategy round 6](reports/tiny-round6.json) | Full-plan A/B for bounded output repair |
| [Tiny strategy rounds 7-9](reports/tiny-round9.json), [8](reports/tiny-round8.json), [7](reports/tiny-round7.json) | Targeted recovery-strategy experiments |
| [Action-budget round 10](reports/tiny-round10.json) | 16 vs 24 tool calls; larger budget rejected |
| [Expanded validation rounds 11-12](reports/tiny-round12.json), [11](reports/tiny-round11.json) | 18-case dataset with per-case threat models |
| [Smaller-budget round 13](reports/tiny-round13.json) | 16 vs 12 tool calls; ~31% token saving |
| [Confirmation rounds 14-15](reports/tiny-round14.json), [15](reports/tiny-round15.json) | Budget replication and rejected context-cap experiment |
| [Stable capability round 16](reports/tiny-round16.json) | 18-case, 2-trial exact/line/reviewed baseline |
| [Settlement round 17](reports/tiny-round17.json) | Read-only settlement after the action budget |
| [Round 4 reviews](reports/round4-reviews.json) | Per-finding verdicts for the round-4 non-exact matches |
| [Round 5 reviews](reports/round5-reviews.json) | Per-finding verdicts for the round-5 non-exact matches |
| [Round 6 reviews](reports/round6-reviews.json) | Per-finding verdicts for the round-6 non-exact matches |
| [Unexpected-finding verification](reports/tiny-unexpected-verification.json) | Deduplicated cross-round catalog of reviewed root causes |
| [Tiny metrics log](reports/tiny-metrics-log.jsonl) | Per-round and per-case execution/metric records for comparison |

The change history is in [CHANGELOG.md](../CHANGELOG.md).
