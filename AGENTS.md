# Project instructions

Read README.md and CONTRIBUTING.md, then inspect git status before making changes.
Preserve existing work. This repository is the canonical home of the server and
all first-party runtime dependencies. Promote settled, tested components
incrementally; do not wait for a complete working server.

Keep research artifacts, raw captures, extracted game content, game binaries,
disassembly, profiles, databases, credentials and private notebook/history out
of commits. Tests here use self-contained constructed data. Never introduce a
build/runtime dependency on a private repository or sibling research checkout.
Record private provenance and reference comparisons in the research workspace.

Use a `codex/` feature branch based on the latest default branch. Commit and push
completed checkpoints and open/update their pull requests. Do not commit or push
directly to the default branch, force-push an open PR or merge without the owner.
Stage explicit paths and review the complete diff before pushing.

Owner override, 2026-10-13: commit locally and test local changes. Do not push or
open a pull request until the change is tested, well documented and working.
This supersedes routine checkpoint publication while investigating a change.

Follow CONTRIBUTING.md for architecture, ownership, dependency and testing rules.
Run all three listed Cargo gates for code changes. State exactly what has been
tested; passing library tests does not establish game or offline-play acceptance.
