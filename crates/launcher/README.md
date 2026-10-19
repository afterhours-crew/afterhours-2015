# nfs-afterhours

Command-line launcher for the Afterhours 2015 local server and, on Windows,
the installed game. It replaces the PowerShell and C# scripts that used to
live under `launcher/`.

The sibling `nfs-afterhours-bundle` package links this launcher and `nfs-server`
into one terminal executable, `Afterhours.exe`. Its server children use the
explicit `--afterhours-server` dispatch flag, recorded as `bundled_server` in
the session request. The same supervisor owns preparation, activation waits,
temporary routing and cleanup. Missing `bundled_server` defaults to false for
existing records; an explicit `--server` uses the standalone-server contract.

Preparation progress goes to stderr and names actual extraction, verification,
cache reuse, account and service steps. JSON results on stdout remain parseable.

```text
nfs-afterhours check   --game-dir <DIR>     verify installation, paths, registration; change nothing
nfs-afterhours setup   --game-dir <DIR>     prepare content and the local account; start nothing
nfs-afterhours serve   --game-dir <DIR>     run the server without a game (every platform)
nfs-afterhours start   --game-dir <DIR>     run the server, start and route the game (Windows)
nfs-afterhours status                       report the current session
nfs-afterhours stop                         ask the live session to close the game, then the server
nfs-afterhours profile [--account-id <ID>]  show a saved driver's profile; change nothing
nfs-afterhours profile --list               list every saved driver in the data directory
nfs-afterhours recover --account-id <ID>    restore account.json for an existing database
```

Every subcommand accepts `--data-dir <DIR>` and `--json`. `--help` on any
subcommand explains it. For installation requirements and the full
walkthrough see [docs/LOCAL-LAUNCHER.md](../../docs/LOCAL-LAUNCHER.md).

Progress and child diagnostics go to stderr; command results go to stdout.
With `--json`, a completed session prints one final status object on stdout,
including `interrupted` if its supervisor died. Errors still have a nonzero
exit code and a message on stderr. Observe an active session with a separate
`status --json` command. Recorded process IDs may be stale; verify process
identity before stopping an orphan after a Unix supervisor crash.

## What it does

A session is one run of `setup`, `serve` or `start`. The front-end command
runs preflight, creates `artifacts/launcher/<session-id>/` in the data
directory, writes `request.json` there and starts a second copy of itself
with the hidden `supervise` subcommand. The supervisor holds the
data-directory lock, on Windows creates its own job object with kill-on-close, and
runs the session state machine:

```text
preparing  -> nfs-server prepare-local, then create-account on first use
starting   -> server ready; game started with the date marker       (start only)
activation -> the installed activation helper shows a window; no timeout
ready      -> routing applied (start) or server listening (serve)
stopping   -> a stop request arrived; close the game, then the server
finished | failed
```

Cleanup always runs after the session body, in a fixed order: close the game
if it is still running, retire the routing lease once the game's lifetime has
ended, stop the server through its stop file, drain both children's log
pumps, disarm the job. A cleanup failure is reported together with any
earlier failure, and the job stays armed so process exit still kills the tree.

## Profiles

`profile` reads one account save, `artifacts/account/<id>.sqlite`, for a
desktop front end; `--list` reads every save there. It never takes the
data-directory lock and never writes, so it runs while the server holds the
save. The save is opened through `nfs-storage`'s read-only inspection: a
read-only SQLite connection with `query_only`, no checkpoint on close, at
most a two-second lock wait, and every domain read in one read transaction,
so a profile is always one committed state. Like any WAL reader, SQLite may
leave empty `-wal` and `-shm` files beside a save it found without them.

Values are decoded by the server's own code in `nfs-services`: identity,
bank balance, REP scores evaluated against the installed thresholds, the
single-row tables (`RepValuesTable`, `PrestigeMedalTable`,
`GameplayStatsTable`, `SpeedListStatsTable`, `ProgressionStats`), the five
garage slots (slot 0 is the primary vehicle), item, livery and table-row
counts, and save metadata. Content comes from `--content-dir`, else the
cache the current session prepared, else the only cache present. Without
content the save still reads; REP, tables and car names are then `null`.

The JSON contract, version 1:

- Every table cell is `{"value": <i32>, "stored": <bool>}`. `stored: false`
  means the save has no cell and `value` is the schema default the server
  serves. Values are the raw stored integers: the game's units for
  distances, times and speeds are not confirmed, so none is applied.
- Only values whose meaning our server defines are formatted: `bank.formatted`
  and `save.updated_at_utc`, next to the raw `bank.balance` and
  `save.updated_at_ms`.
- Item IDs and the persona ID are decimal strings, so JavaScript keeps every
  digit. Definition GUIDs are 32 lowercase hexadecimal characters.
- A car's `vehicle.label` is its blueprint asset name without `Car_` and with
  spaces for underscores, not the game's localized display name.
- A domain that is absent or cannot be decoded is `null` and adds a
  sentence to `warnings`. A save that cannot be opened at all is an error
  (exit 1, 4 or 5); in `--list` it is an entry with `error` set.

## Why it is built this way

**Two processes.** The supervisor runs separately from the command you typed
so your terminal is never placed in the session job, and so a supervisor
crash is handled by Windows closing the whole tree rather than by code that
just crashed.

**Inherited Windows jobs.** Windows may place even a desktop-launched process
in a job. The supervisor creates a new, empty job nested under any inherited
job, retaining the parent's restrictions. Its cleanup owns only the supervisor
and its descendants, not ancestor or sibling processes. Incompatible job limits
produce a Windows error before children start. Windows subprocess regressions
exercise normal cleanup and a forcibly terminated supervisor, verifying that
session children exit while an unrelated sibling remains alive. These tests do
not launch the game or validate activation.

**Decisions are portable, actions are not.** The state machine in
`supervisor.rs` only decides what to do next. It calls a `Steps` trait, which
the live session implements with real processes and the unit tests implement
with a recording fake. That is how ordering, cancellation at every wait and
cleanup after every failure are tested on any machine without a game.

**One crate may use `unsafe`.** `nfs-afterhours-sys` wraps the Win32 and
libc calls behind safe functions with bounded buffers, and Clippy requires a
`SAFETY` comment on every block. This crate contains no `unsafe` at all.

**Windows first, not Windows only.** `serve` works on Linux and macOS today.
Only game launch, the routing lease that writes 13 bytes into the game
process, the activation-helper window check and the registry read are
Windows-specific, and each sits behind `cfg(windows)` in `platform/`. There
is deliberately no kernel-enforced child cleanup on Unix: `serve` has no game
and no memory lease, so a supervisor crash leaving the server running is
ordinary daemon behavior, and `status` reports it as `interrupted`.

**Fail closed.** The installation must match the supported build by digest.
The routing lease pins the game by handle, creation time, image path and
digest, verifies three code regions by hash, checks the server still owns the
loopback listener, and writes with read-back verification and rollback that
never overwrites bytes it did not write. Any mismatch stops the session and
says so.

**Nothing is hidden in the data directory.** Every file the launcher writes
is small, versioned JSON (`account.json`, `current-session.json`,
`request.json`, `status.json`), a bounded log, or a digest-named copy of our
own server. The formats are the ones the PowerShell scripts wrote, so
existing data directories keep working.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | refused by a safety or precondition check; nothing was changed |
| 2 | invalid command line |
| 3 | a session started and then failed; inspect its session directory |
| 4 | no live session or record to act on |
| 5 | input/output or internal error |

## Layout

| Module | Owns |
| --- | --- |
| `cli` | clap command tree and help text |
| `commands/` | one function per subcommand; `launch` holds the front-end and `supervise` |
| `preflight` | the read-only checks before anything is written or started |
| `supervisor` | the state machine; `supervisor/live` implements it over processes |
| `platform/` | the `Platform` trait, the shared child with bounded log pumps, and the Windows and Unix implementations; `platform/windows/lease` is the routing lease |
| `endpoint` | the 13-byte transaction over abstract memory, tested with injected faults |
| `profile` | read-only profiles from account saves; `profile/content` picks and loads the prepared content |
| `paths`, `records`, `session`, `install`, `digest`, `compat`, `marker`, `clock` | portable pieces with their own unit tests |

## Testing

```sh
cargo test -p nfs-afterhours -p nfs-afterhours-sys
```

Unit tests need no game, server or network. The integration tests in
`tests/cli.rs` drive the built binary; on Unix they run a complete `serve`
session against a shell script standing in for the server, observe it from a
second process through `status`, stop it, and also kill a supervisor to see
`interrupted` reported. The Windows process-layer tests run in CI on a
Windows runner. Live acceptance on the supported installation is recorded in
`docs/LOCAL-LAUNCHER.md`, not claimed by tests.
