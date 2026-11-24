---
name: rtk
description: Use the project-local RTK CLI to compact verbose shell output from routine builds, checks, tests, and searches. Use raw commands when exact output or full diagnostics are needed.
---

# RTK

From the project root, prefix supported commands with `./.agents/bin/rtk`:

- `./.agents/bin/rtk cargo test --workspace`
- `./.agents/bin/rtk npm run check`
- `./.agents/bin/rtk rg -n 'pattern' src`

Use `./.agents/bin/rtk --help` for supported commands. Run the original command if RTK is unavailable, its summary omits needed details, or the user asks for exact output. Use the underlying command's exit status to decide whether a check passed.

The project-local binary is [RTK OSS v1.2.1](https://github.com/rtk-ai/rtk/releases/tag/v1.2.1) for Linux x86-64. For another platform or checkout, use the corresponding verified release binary at `.agents/bin/rtk` or an existing `rtk` on PATH.
