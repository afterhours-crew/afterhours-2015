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

Owner override, 2026-10-14: use local changes for fast testing and push checkpoint
commits to the feature branch for backup. Do not open a pull request until the
entire requested tracking issue, or the sub-issue you were asked to deliver, is
implemented, tested and documented.

Follow CONTRIBUTING.md for architecture, ownership, dependency and testing rules.
Run all three listed Cargo gates for code changes. State exactly what has been
tested; passing library tests does not establish game or offline-play acceptance.

For story work, follow [STORY-WORKFLOW](docs/STORY-WORKFLOW.md). Present a bounded
gameplay batch for owner approval before implementation; after approval, complete
its research, implementation, checks and review preparation autonomously. Routine
fixes within that scope do not need repeated approval. Keep new scope, live
acceptance, merging and releases distinct. Do not copy private capture material
into public plans or tests.

Remaining gameplay systems are tracked in GitHub issues, not in docs. The pinned
[gameplay roadmap][roadmap] holds the priority order; each system has a
`kind:tracking` issue with a `system:` label that owns its acceptance list.
Work against that issue and reference it in commits and pull requests. When you
find an unknown, bug or extra task, open a new issue with the same `system:`
label and a `kind:` label (`feature`, `research`, `live-check` or `bug`), and
add it as a sub-issue of the tracker. Tick acceptance items only after a live
check in the game and state exactly what was tested; do not fold unknowns into
documentation or treat library tests as game acceptance.

[roadmap]: https://github.com/afterhours-crew/afterhours-2015/issues/62
