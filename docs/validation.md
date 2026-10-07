# Validation: 2026-10-07

The live evaluation used credentials from `/home/ubuntu/.codex/deepseek.config.toml`, read privately and passed as `AUDIT_API_KEY` to the harness process. No credential was printed, persisted in this repository, or added to command arguments. The local profile was left unchanged. The model selection was `deepseek-flash`; its provider URL was adapted to `https://api.deepseek.com/chat/completions` for a plain LLM request.

Run configuration:

```sh
./target/debug/audit-harness evaluate \
  --dataset datasets/smoke/dataset.json \
  --endpoint https://api.deepseek.com/chat/completions \
  --model deepseek-flash --jobs 2 --timeout-ms 120000 \
  --output deepseek-evaluation.json
```

The credential environment must be supplied privately before reproducing this command; choose a new output filename because the harness refuses overwrites. Defaults: 256 KiB source context, 4,096 maximum requested output tokens, 1 MiB stdout/stderr limits. Each of the six cases made one request. No retries or output repairs were performed, and the prompt did not include ground truth. The run did not override DeepSeek's default thinking setting.

| Result | Value |
| --- | ---: |
| Successful cases | 6 / 6 |
| Exact-match cases | 6 / 6 |
| True positives | 3 |
| False positives | 0 |
| False negatives | 0 |
| Precision / recall / F1 | 1.0 / 1.0 / 1.0 |
| Total wall-clock duration | 10.702 s |

All three corrected counterparts produced no findings. The unsafe shell, SQL, and eval cases produced the expected CWE, path, and sink line. The full findings and individual timings are in [deepseek-evaluation.json](../deepseek-evaluation.json).

This is one run on six small handcrafted cases with three vulnerability families. It establishes working provider integration and correct results on the supplied smoke set. It does not establish general vulnerability discovery effectiveness, repeatability, calibrated severity, or robustness to prompt injection. A broader held-out dataset and repeated measurements remain the appropriate next validation step. Token usage and cost were not captured.

Local checks passed with stable Rust 1.99.0:

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
11 integration tests passed
```

Tests cover strict response schemas, matching and duplicate suppression, failed-case scoring, dataset validity, source bounds, nested symlink exclusion, report overwrite protection, process failures and excessive output, deadline/cancellation descendant cleanup on Linux, deterministic demo evaluation, HTTP authentication/request scoping, API failures, truncated completions, and out-of-snapshot findings.

SHA-256 fingerprints at validation:

```text
f0b9d369fab9d6983948c48604e5b1ee7f84b73e5ce4ef8fe2b57fb1d5d67b98  Cargo.lock
e5a232d5710d050ed6090c5f3f9888ccc1b4d905fcfb45da174986e81ef20d52  prompts/audit.txt
78ad6f862ea38f31ebced76663fed2483c3856990829e2cdf0ce765d4e1e49db  datasets/smoke/dataset.json
aae6cc3c771f9ed152b4e44ab5bfe1b89c8abdbd2e770b448aaf6ea52f9967a1  datasets/smoke/sources/command_safe.py
0bddaa2baf3f9d6b80270ad8927d9ac01b0ebec1f879bb153b1614df8e780575  datasets/smoke/sources/command_unsafe.py
9ce6d0974781dcb7840c2e08ed60b2a6faf1c4ea2109690dd4fee9a8747c0597  datasets/smoke/sources/eval_unsafe.py
4983c0ca231983478b4b0d43127a5ef159f5df5c1ed0ef6ecbfdeb630ce66b9b  datasets/smoke/sources/literal_safe.py
eb22131678fc7dca12d25da9f4cfc9a640bdec081eae2f9f38a297ee5ef7b9ad  datasets/smoke/sources/sql_safe.py
65ebcd527f8cd77f70ccaa8c0fd000b845713d3c27fbaadcd5c922ebead89a43  datasets/smoke/sources/sql_unsafe.py
```
