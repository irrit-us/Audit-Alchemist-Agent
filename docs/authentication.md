# Authentication

The built-in agent supports two credential sources, selected with `--auth`:

- `api-key` (default): a bearer token read from an environment variable, sent to
  a chat-completions endpoint.
- `codex`: Sign In With ChatGPT, reusing a login written by the OpenAI Codex CLI
  against the Codex Responses backend.

Tokens are never command-line arguments, log lines, or report fields.

## API key

Put the bearer token in an environment variable using your normal secret
management, then select a model and a full chat-completions endpoint:

```sh
export AUDIT_API_KEY=...
cargo run --locked -- audit \
  --root datasets/smoke --target sources/command_unsafe.py \
  --instruction 'The name argument is attacker-controlled. Audit command execution.' \
  --endpoint https://YOUR-PROVIDER/v1/chat/completions \
  --model YOUR-MODEL
```

`--api-key-env` selects a different variable name. Loopback HTTP endpoints are
supported for local models; set the variable to a nonempty placeholder if the
local server ignores authentication. A compatible server accepts `model`,
`messages`, `temperature`, and `max_tokens`, and returns one
`choices[].message.content` string with `finish_reason: "stop"`.

## Sign In With ChatGPT (Codex)

A ChatGPT subscription can power the auditor through the credential store of the
official [Codex CLI](https://github.com/openai/codex). Sign in once, then select
the Codex backend:

```sh
codex login
cargo run --locked -- audit \
  --root datasets/smoke --target sources/command_unsafe.py \
  --auth codex --model YOUR-CODEX-MODEL \
  --instruction 'The name argument is attacker-controlled. Audit command execution.'
```

`--auth codex` reads `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`),
reuses the access token while it is valid, and refreshes it through the public
OAuth token endpoint when it is within two minutes of expiry. It sends a
Responses-API request to `https://chatgpt.com/backend-api/codex/responses` with
the `chatgpt-account-id` account header, parses the Server-Sent Events stream,
and validates the same versioned findings JSON. `--codex-auth-file` selects a
different credential file (must be absolute, because the agent runs with the
source root as its working directory) and `--codex-base-url` overrides the
backend base URL.

### On-disk format

The file uses the documented Codex layout:

```json
{
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "...",
    "access_token": "...",
    "refresh_token": "...",
    "account_id": "..."
  },
  "last_refresh": "2026-10-08T00:00:00Z"
}
```

The account id is read from `tokens.account_id` or from the
`https://api.openai.com/auth` JWT claim (`chatgpt_account_id`). The JWT payload
is decoded only to read `exp`; signature verification is the server's
responsibility.

### Credential handling

- A refresh updates only the token fields, preserves every other key, runs under
  an advisory lock so concurrent workers share one refresh, and is written back
  atomically with owner-only permissions on Unix. Refreshing a token is the only
  operation that mutates the file.
- Credentials render as `<redacted>` in debug output and never enter reports or
  command arguments.
- An HTTP 401 triggers one forced refresh and a single retry as authentication
  recovery, separate from the bounded transport retries described in
  [Configuration](configuration.md).

Using a ChatGPT subscription outside the official client is your responsibility
under the provider's terms.
