# Terminal tester bundle

`cargo build --release -p nfs-afterhours-bundle --locked` builds `Afterhours.exe`
on Windows. It links the existing launcher and server libraries; no game content
or separate server payload is embedded. With no arguments it starts against the
game beside the executable. Explicit commands preserve the CLI interface.

The launcher and server still run as separate supervised processes using this
same executable. Server children explicitly select `--afterhours-server`; this
also works for the verified SDK compatibility copy. File names do not select
roles. The frontend stays outside the supervisor's Windows job.

`cargo test -p nfs-afterhours-bundle --locked` checks argument selection and real
subprocess dispatch. Content progress tests cover actual constructed extraction,
verified reuse and failure without false completion. These tests do not prove
live game or activation acceptance.

Initial validation (2026-10-18): workspace format, Clippy and 1,589 tests pass.
Read-only preparation against the supported installation reports all 14 content
categories, and a second run verifies/reuses the same cache. The bundled game
launch and activation interaction have not yet been revalidated live; existing
CLI acceptance does not establish that additional scope.

## Release packaging

`.github/workflows/release.yml` runs on a **published** GitHub release, including
prereleases, checks out the release tag, runs the workspace gates, builds for
`x86_64-pc-windows-msvc` with the static C runtime, and runs
`tools/Package-TerminalBundle.ps1`. The script copies only the executable and
explicit documentation/license files, tests server/launcher dispatch from the
isolated package directory, and writes a ZIP, SHA-256 checksum and source/build
manifest. It collects dependency notices from locked Cargo package sources.
No game, account, research file or locally generated content is a package input.

The workflow attaches those files to the triggering release. Pull requests
changing the packaging and manual workflow runs build Actions artifacts only;
they neither create nor change a release. Pull requests use the workspace gates
in `ci.yml`; release and manual runs repeat the gates before packaging.
Existing release assets are never overwritten on rerun. Resolve any partial
upload deliberately. Historical source-only releases remain unchanged.

Source/activation scope and tester instructions: [quickstart](../../docs/TESTER-QUICKSTART.md).
