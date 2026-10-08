---
name: tmux-debugging
description: Operate an installed tmux session for local interactive debugging with explicit pane targeting and bounded output capture.
---

# tmux debugging

Check command -v tmux. Prefer debugger batch mode when sufficient. Bash calls have no interactive stdin and descendant processes are cleaned up when a call ends: do not assume a tmux server started by a tool survives subsequent calls.

For a new debugging session, run its complete lifecycle in a single Bash call with a unique private socket (-L), a detached session, and a trap that cleans up only that private server. Capture the exact pane ID using -P -F '#{pane_id}'. Send literal command text using send-keys -t "$pane" -l 'command', then send Enter separately. Use bounded readiness polling and capture-pane -p -S -100 -t "$pane" to inspect output before the next step. All commands must select the same private socket.

For an existing user session, inspect list-panes -a -F '#{session_name} #{pane_id} #{pane_current_command}' and capture the intended pane before sending any input. Use its exact pane ID. Do not interrupt another process or kill a shared server. Avoid global pane selection, broad send-keys, or commands based on stale screen state.

Consult tmux list-commands for the specific command if syntax is uncertain. Capture only the lines needed to decide the next step, and finish within the Bash timeout.

Adapted from 0RAYS/codex-auditor tmux-operator; see ../LICENSE.codex-auditor and ../UPSTREAM.md in the source distribution.
