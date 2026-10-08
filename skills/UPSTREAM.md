# Built-in skill sources

The native-debugging, tmux-debugging, and finding-review skills adapt selected techniques from [0RAYS/codex-auditor](https://github.com/0RAYS/codex-auditor/tree/5974e700bf5b44f10d885bb238dd8bcab8f42145/skills), pinned at commit `5974e700bf5b44f10d885bb238dd8bcab8f42145` (MIT, copyright 2026 RcoketDev). The upstream license is retained in LICENSE.codex-auditor.

Adaptations use this harness's actual tools, deadlines, process cleanup, and JSON protocol. Optional debugger installations, fixed report paths, upstream scoring fields, large manuals, and external scripts are not bundled. code-audit and poc-validation are original guidance for this harness.

The Foundry, GDB, Node Inspector, and pwntools helpers are original implementations against their documented APIs. Their skill entrypoints link the relevant upstream documentation; no debugger distribution or forge-std checkout is bundled.
