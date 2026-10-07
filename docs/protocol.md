# Version 1 contracts

An external agent receives one JSON object followed by a newline on stdin; EOF follows immediately. It runs with the dataset directory as its working directory. It must exit zero and write exactly one JSON response on stdout. Diagnostic output belongs on stderr. Unknown fields and unsupported schema versions are rejected.

Request:

```json
{
  "schema_version": 1,
  "case_id": "command-injection",
  "target": "sources/command_unsafe.py",
  "instruction": "The name argument is attacker-controlled. Audit command execution."
}
```

Response:

```json
{
  "schema_version": 1,
  "findings": [
    {
      "cwe": "CWE-78",
      "path": "sources/command_unsafe.py",
      "line": 5,
      "severity": "high",
      "title": "Command injection",
      "evidence": "Attacker-controlled name is concatenated into a command executed with shell=True."
    }
  ]
}
```

The findings array is required, even if empty. Severity is one of `info`, `low`, `medium`, `high`, `critical`. Paths are normalized POSIX paths relative to the source root; absolute paths, `..`, `.`, backslashes, colons, and empty components are invalid. CWE uses `CWE-` followed by a positive decimal identifier without leading zeros. Titles are 1–1,024 bytes and evidence 1–16,384 bytes, excluding all-whitespace strings. At most 10,000 findings are permitted. The built-in LLM adapter additionally checks paths and line numbers against the actual snapshot. External adapters are responsible for ensuring findings correspond to source; the harness checks response structure and scores all reported keys.

Dataset (paths relative to the dataset file's directory):

```json
{
  "schema_version": 1,
  "name": "example-v1",
  "cases": [
    {
      "id": "command-injection",
      "target": "sources/command_unsafe.py",
      "instruction": "The name argument is attacker-controlled. Audit command execution.",
      "expected": [{"cwe": "CWE-78", "path": "sources/command_unsafe.py", "line": 5}]
    }
  ]
}
```

Dataset files are limited to 4 MiB and 1–10,000 cases. IDs are unique, at most 128 ASCII alphanumeric/underscore/hyphen characters. Instructions are nonempty and at most 64 KiB. All targets must exist within the canonical dataset root. Expected source paths must be files within their case target; labels must be unique and their lines must exist. An empty expected array represents a clean target. Ground truth is evaluation metadata and is not part of the agent request.

Reports use schema version 1, ordered case results, invocation metadata, aggregate metrics, and case-level matched/unexpected/missed keys. Outcomes distinguish `success`, `timeout`, `spawn_error`, `io_error`, `output_limit`, `nonzero_exit`, and `invalid_response`. Failed invocations contribute no accepted findings. No retries or hidden repairs occur after invalid JSON.
