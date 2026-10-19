# Terminal tester bundle

`cargo build --release -p nfs-afterhours-bundle --locked` builds `Afterhours.exe`
on Windows. It links the existing launcher and server libraries; no game content
or separate server payload is embedded. With no arguments it starts against the
game beside the executable. Explicit commands preserve the CLI interface.

Windows builds embed the supplied Afterhours icon from
`resources/afterhours.ico`, including all seven sizes (16–256 pixels). It is
compiled with the Windows SDK resource compiler and linked into the executable;
users do not need a separate icon file. Changing the ICO triggers a rebuild.
Non-Windows builds skip the resource step. Missing Windows resource tools fail
the build instead of silently omitting the icon.

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

`.github/workflows/release.yml` runs when a version tag is pushed, or when a
maintainer manually selects an existing tag. It checks the tag and bundle
version, runs the workspace gates, builds for
`x86_64-pc-windows-msvc` with the static C runtime, and runs
`tools/Package-TerminalBundle.ps1`. The script copies only the executable and
explicit documentation/license files, tests server/launcher dispatch from the
isolated package directory, and writes a ZIP, SHA-256 checksum and source/build
manifest. It collects dependency notices from locked Cargo package sources.
No game, account, research file or locally generated content is a package input.

The workflow verifies the remote tag still points to the built commit, then
creates an **unpublished draft** with those files. A maintainer reviews and
publishes the finished draft; publication does not rebuild. See the
[release guide](../../docs/RELEASING.md) for version tags, preparation and retries.
Pull requests and manual runs without `release_tag` create development Actions
artifacts only. Pull requests use the workspace gates in `ci.yml`; tag and manual
runs repeat the gates before packaging. Draft-publication guards have portable,
offline tests in `tools/Test-TerminalDraft.ps1`.
Existing releases and assets are never overwritten. Resolve any partial draft
upload deliberately. Historical releases remain unchanged.

Source/activation scope and tester instructions: [quickstart](../../docs/TESTER-QUICKSTART.md).
