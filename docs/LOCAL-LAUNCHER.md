# Run the local game and server

`nfs-afterhours` is the command-line launcher for Afterhours. On Windows it
prepares content from the supported installation, creates a separate local
account, starts the server and the game in one ordinary-user session, routes
the game to the local server and cleans both up. On Linux and macOS it does
everything except start the game: `nfs-afterhours serve` runs the server
against a copied installation directory. It needs no research repository,
private scripts, copied profile or captured service replies, and no PowerShell.

**Development preview:** the Rust launcher has passed an ordinary-user Windows
run through the menu, saved garage and free roam, followed by `stop` and process
cleanup. Continuation after owner-completed activation also reached the splash.
Cold disconnected startup, prolonged activation waiting and activation cancellation remain untested;
see [validation](#developer-validation-and-current-limits). Complete progression
remains open.

For releases containing `Afterhours-windows-x64.zip`, use the
[terminal tester quickstart](TESTER-QUICKSTART.md). Its `Afterhours.exe` bundles
these same server and launcher implementations. With no arguments it starts
using the game folder beside the executable and keeps the terminal open after
completion or failure. It prints the real extraction stages and cache reuse,
and keeps installed activation requirements explicit. Game data is generated
locally; none is shipped. Existing source-only checkpoints have no binary bundle.

The build instructions below are for contributors and users of the separate
`nfs-server` and `nfs-afterhours` tools. All source lives in this repository.

## Install the build tools

For the Windows game-and-server path, install these tools first:

| Tool | Required setup |
| --- | --- |
| [Git for Windows](https://git-scm.com/install/windows) | Provides `git` for cloning and updating this repository. |
| [Rust via rustup](https://rustup.rs/) | Use the **x86_64-pc-windows-msvc** host. This checkout's `rust-toolchain.toml` selects Rust **1.98.1**, rustfmt and Clippy; rustup downloads them when first needed. |
| [Visual Studio C++ tools](https://rust-lang.github.io/rustup/installation/windows-msvc.html) | Select **Desktop development with C++**, including the MSVC x64/x86 compiler and a Windows SDK. Visual Studio Build Tools is sufficient; the Visual Studio IDE is optional. |
| [Strawberry Perl](https://strawberryperl.com/) | Put `perl.exe` on your user `PATH`. The vendored OpenSSL build uses it. |
| [NASM](https://www.nasm.us/) | Put `nasm.exe` on your user `PATH` for the OpenSSL assembly build. |

On Linux or macOS, install rustup, a C compiler and Perl; that is enough for
`serve`.

Reopen your terminal after installing tools or changing `PATH`. Installing the
compiler/SDK may require an administrator; building and running Afterhours use an
ordinary user account. The launcher rejects an elevated session. There is no
separate OpenSSL or SQLite service to install: Cargo builds those libraries from
the locked dependencies. No Python, Node.js, PowerShell 7 or private research
tools are required.

Internet access is needed initially to obtain the source, Rust toolchain and Cargo
dependencies. After caching them, Cargo accepts `--offline` for builds. That build
cache is separate from the installed game's activation state described below.

## Requirements

- A legitimately installed, previously activated Need for Speed (2015) PC game.
  Keep the existing installation and license cache. Fresh activation, expired
  caches and running after uninstalling all EA software are not established.
- For `start`: 64-bit Windows, running without elevation. The bundle can be
  double-clicked in File Explorer; the CLI also works in Windows Terminal,
  Command Prompt or PowerShell. An inherited Windows job is supported when
  Windows permits a nested session job. `serve` also runs on Linux and macOS
  as an ordinary user, not root.
- For `start`: existing 32-bit `HKLM\SOFTWARE\Origin\ClientPath` registration.
  The tested registration names `EADesktop.exe`. `Origin.exe` compatibility is
  implemented but has not been tested live. Other names are refused.
- Built `nfs-server` and `nfs-afterhours` executables, following
  [Build from source](#build-from-source). Running them does not invoke Git,
  Cargo, Perl or NASM.
- For `start`: close the EA app and any running copy of the game. Local TCP
  port 3216 must be available. Afterhours reports conflicts and leaves the
  other process alone.

| Installed file | Accepted SHA-256 |
| --- | --- |
| `NFS16.exe` | `92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df` |
| `Core/ActivationUI.exe` | `5901b0ad269e12deecc2bcd2eaa56f8964538a8d6b1a265651d56dbd636daefd` |

Other builds fail before anything starts. Do not replace a digest to bypass a
failure: content mappings and the routing lease also depend on that exact
executable. `serve` checks `NFS16.exe` only; the activation helper matters only
when the game is started.

## Build from source

Open an ordinary, unelevated terminal and clone the public repository:

```sh
git clone https://github.com/Afterhours-Crew/afterhours-2015.git
cd afterhours-2015
```

Check the tools before building:

```sh
rustc --version   # 1.98.1, selected by this checkout
rustc -vV         # host: x86_64-pc-windows-msvc on Windows
cargo --version
perl -v
nasm -v           # Windows only
```

Build the release server and launcher from the repository root:

```sh
cargo build --release -p nfs-server -p nfs-afterhours --locked
```

The first build also compiles OpenSSL and SQLite and can take several minutes.
The executables are `target/release/nfs-server` and `target/release/nfs-afterhours`
(with `.exe` on Windows). The launcher looks for `nfs-server` next to itself,
which is where this build puts it. If you set `CARGO_TARGET_DIR`, use a custom
Cargo target or move the executables apart, pass the server's full path with
`--server` to `check`, `setup`, `serve` and `start`.

If Cargo reports a missing linker, `cl.exe`, `nmake` or Windows SDK library, repair
the C++ workload/SDK installation. For missing `perl` or `nasm`, fix their user
`PATH` and reopen the terminal. For a build that cannot discover an installed
compiler, use Visual Studio's **x64 Native Tools Command Prompt** to run Cargo;
return to an ordinary terminal for game startup. Do not change the pinned
toolchain or dependency versions merely to get past a setup error.

## Start

From the same checkout, with the build complete:

```sh
./target/release/nfs-afterhours start --game-dir "F:\EA Games\Need for Speed"
```

Use your actual installation path. The default data directory is
`%LOCALAPPDATA%\Afterhours2015`; `--data-dir "D:\My Afterhours data"` selects
another dedicated local directory. Keep it outside the game installation.
Spaces and Unicode characters are supported. Junctions, symlinks, network
shares and drive roots are refused.

The first start generates and validates content, creates an independent identity
and saved database, starts loopback services, and opens the game. Subsequent starts
reuse verified content and the same account. Preparation can take a minute or
more. The command stays in the foreground and prints preparation, server
diagnostics and session status in the same terminal. Wait for **Ready to play**
before pressing Space in the game. You can also check from a second terminal:

```sh
nfs-afterhours status
```

Pass the same `--data-dir` to every command if you selected a custom location.
**Ready to play** means server listeners and guarded routing are ready; it does not mean the
game has logged in or reached a playable world. Continue through the normal game
menus yourself. The launcher never presses keys or accepts agreements.

If the installed activation helper opens a sign-in window, the launcher prints
**EA activation needs your attention** and waits without a startup timeout. It does not read or
submit credentials. This is EA's activation requirement, separate from your local
Afterhours account; local play is not ready while it waits. You can cancel with
`nfs-afterhours stop` from another terminal. If activation replaces the original
game process, this session ends with an error; start again after activation. It
never adopts a different game process without the original identity guards.

`--player-name "My Driver"` sets the first account's display name. It does not
rename or replace an existing account. New accounts start **after the scripted
prologue**; the official prologue and complete progression are not yet supported.
Launcher-created tester accounts start one level below the installed REP cap
(level 69 of 70 on the supported build), with $10,000,000. These resources are
committed with the new account; existing accounts are reused without a top-up.
This is a local testing preset, not earned rewards or completed career events.
Stage headings and labeled colors distinguish progress, success, activation waits
and errors. Redirected output and `NO_COLOR=1` remain plain; JSON is uncolored.
The local challenge menu uses three fixed preview objectives derived from installed
type metadata. It grants no challenge rewards and does not implement rotating
schedules or challenge completion. No remote product entitlements are granted.

**Traffic and rivals.** The amount of ambient traffic and the number of roaming
rivals are yours to choose; the defaults are the original game:

```sh
# Half the original traffic and at most two rivals at once.
nfs-afterhours start --game-dir "F:\EA Games\Need for Speed" --traffic 50 --rivals 2
# Back to the original amounts.
nfs-afterhours start --game-dir "F:\EA Games\Need for Speed" --traffic 100 --rivals original
```

`--traffic` is a percentage of the original traffic from 0 (none) to 300.
`--rivals` is the most roaming rivals active at once, from 0 (none) to 128, or
`original` for the original number. Both are saved in `settings.json` in the data
directory and apply to every later `start`, `serve` and `setup` until changed. You can
also edit that file directly; the start output shows the values in use:

```json
{"traffic_percent": 50, "rival_limit": 2}
```

`rival_limit` set to `null` means the original number. Out-of-range, unknown or
malformed settings are refused rather than ignored. More traffic or rivals than the
original costs client performance.

Every subcommand accepts `--json` to print one JSON object instead of text, and
exits with a documented code (`nfs-afterhours --help` lists them): 0 success,
1 refused with nothing changed, 2 bad command line, 3 session failed, 4 nothing
to act on, 5 input/output error.

## Check, setup, serve and stop

```sh
# Read-only: installation hashes, path safety, registration and port/process conflicts.
nfs-afterhours check --game-dir "F:\EA Games\Need for Speed"

# Generate content and account without starting the game or listeners.
nfs-afterhours setup --game-dir "F:\EA Games\Need for Speed"

# Run only the server, on any platform; stop with Ctrl+C or `nfs-afterhours stop`.
nfs-afterhours serve --game-dir /mnt/games/nfs2015

# Close this session's game, then its server.
nfs-afterhours stop
```

`serve` prepares like `setup`, starts `nfs-server` directly on loopback and
supervises it until it exits, a stop is requested or you press Ctrl+C. On Linux
and macOS point `--game-dir` at a copy of the installation directory; the
content readers need its files, not a running game. The server leaves the LSX
port disabled in this mode. Non-loopback listening for a remote client is not
yet implemented and is tracked separately.

Quitting normally in the game also shuts down the server. Stop first requests
normal game closure, then terminates only its owned process tree if closure does
not complete within 15 seconds. A single data-directory lock prevents concurrent
supervisors. Game/process and port checks reject other active sessions.

The command starts a second copy of itself as the session supervisor in the same
console and waits for it to finish. No shortcuts, desktop automation or
background launcher app are needed. On Windows the supervisor puts itself and
its children in a job with kill-on-close: if the supervisor is forcibly
terminated, Windows closes that owned process tree. If the server dies, the
supervisor closes the game. Process handles and creation identities prevent
cleanup from targeting a reused PID. The memory lease lasts for game lifetime;
active play has no experiment timer. On Linux and macOS there is deliberately
no equivalent: `serve` has no game to protect, so a supervisor that dies leaves
an ordinary server process behind, and `status` reports the session as
`interrupted` with the server's PID so you can stop it. Established idle LSX
connections remain open; incomplete handshakes/frames and writes still have
deadlines. Reboot leaves no registration or service change to undo. SQLite
crash recovery runs on the next start; this is not a promise against hardware
failure or an unverified power-loss scenario.

## Data and recovery

Keep `account.json` and `artifacts/account/` together. Stop the session before
backing up the entire data directory, including SQLite journal files if present.
The launcher never resets an existing account or imports an EA save. The data
directory layout and record formats are the ones the previous PowerShell
launcher used, so an existing directory keeps working.

`status` reports `interrupted` when a previous live status has no supervising
lock. Start again after its processes have exited. If setup was interrupted after
the account database was committed but before `account.json` was written, preserve
the directory and recover the small pointer using the account ID from the
successful creation output or the `<id>.sqlite` filename:

```sh
nfs-afterhours recover --account-id <32 lowercase hex digits>
```

Recovery refuses an existing different pointer and never edits the database.
The server still validates schema and ownership when starting. If no complete
account database exists, keep the failed directory for diagnosis and choose a new
data directory. Do not overwrite it with someone else's profile.

Configuration generations and executable copies are immutable and hash checked.
A failed copy/preparation may leave a `.partial` directory/file. It is not used on
later starts. A modified published configuration or executable is refused; inspect
it and preserve your account before deciding what to remove. No automatic data
pruning or diagnostic upload is performed.

## What changes on the machine

The launcher reads existing EA registration but does not write the registry,
change/start/stop services, change firewall/DNS/hosts settings, install a driver,
request UAC, modify game files or disable Windows protections. Run it without
administrator privileges; elevated invocation is rejected.

For the supported SDK's process-name check, it makes a verified copy of **our own
server executable** in `data/bin/<server-sha256>/EADesktopDebug.exe` (or the
untested Origin variant). This is not an EA executable. The SDK recognizes the
registered client's basename plus `Debug`, allowing the actual EA app to remain
closed. This compatibility route still requires existing registration and cached
activation. It does not implement or supply licenses. `serve` runs `nfs-server`
directly and makes no copy.

The routing lease changes 13 startup data bytes in the owned game process. It
checks executable identity, live code guards, root thread/TLS/provider shape,
original bytes and the owned server's loopback listener. It verifies writes and
can reverse partial writes without overwriting another writer's data. If the game
has already consumed its startup routing, preparation fails closed. Once the
pinned game process exits, its temporary address space is gone. No executable
instructions, disk patches, debugger attachment or broad process injection API
are exposed. All operating-system calls live in one crate,
`nfs-afterhours-sys`, behind safe wrappers; the launcher itself contains no
unsafe code.

## Diagnostics and troubleshooting

Each session gets a unique `artifacts/launcher/<session-id>/` containing
`request.json`, `status.json` and bounded stdout/stderr logs. Default server
recordings contain route metadata and lengths, not request payloads. Each log
file retains at most 4 MiB; the server recording has a 64 MiB, 100,000-event
and 256-stream budget. It stops recording at the limit and continues serving.
These bounds are per run; old sessions are retained for deliberate user cleanup.

Unimplemented operations produce `WARN unsupported operation received` in the
terminal and `server.stderr.log`, including safe route identifiers and repeat
counts. Connection summaries include suppressed observations. See the
[diagnostic fields and limits](../crates/server/README.md#unsupported-operation-diagnostics).
Raw packet capture is unnecessary for these warnings.

`--record-protocol` explicitly enables local raw diagnostic capture. Those files
can contain identities and local authentication inputs. Review/redact them before
sharing; do not publish saved databases, activation files or extracted game data.
There is no upload/export feature that silently sends them anywhere.

| Result | Next step |
| --- | --- |
| Unsupported hash | Use the supported installation; do not weaken guards. |
| Existing game/EA process or occupied port 3216 | Close the conflicting application yourself, then retry. |
| Missing/unrecognized registration | This installation is outside the supported launch path. No automatic repair or admin fallback runs. |
| Windows could not create an isolated session job | The accompanying Windows error identifies the failed operation. A parent application may impose incompatible job restrictions; retry from File Explorer or an ordinary terminal. Afterhours preserves parent restrictions and refuses before starting children if isolation fails. |
| Exit code 1 before a session starts | Read the refusal on standard error; nothing was changed. |
| Exit code 3 | Read the session's `status.json` and logs in the printed session directory. Preserve existing state. |
| `activation` | Respond to the installed game's dialog yourself, or cancel with `stop`. Launcher-free operation still depends on valid installed activation state. |
| `interrupted` | The supervisor is gone. On Windows its children were closed by the job; on Unix stop the listed server PID yourself. Start again to recover. |
| Game exits with a nonzero code | The status records the code; inspect Windows Application events and local logs. A crash is not a successful launch. |

## Developer validation and current limits

Run the normal three Cargo gates plus the launcher's own tests:

```sh
cargo test -p nfs-afterhours -p nfs-afterhours-sys --locked
```

Unit tests cover the path rules, record formats, digests, the verified server
copy, the lock and stop protocol, the launch marker vectors, the 13-byte
transaction with injected partial writes, the bounded child log pumps, and the
session state machine against a recording fake: every state, cancellation at
every wait, every failing step and the fixed cleanup order. Integration tests
drive the built binary; on Unix they run a complete `serve` session against a
script standing in for the server, watch it from a second process through
`status`, stop it, and kill a supervisor to see `interrupted`. The Windows
process-layer tests (job object, process pins, thread lookup, memory access,
TCP ownership, registry) run in CI on a Windows runner.

On 2026-10-16 the Rust launcher passed `check`, content preparation and `start`
on the supported Windows installation without administrator access. The game
reached the menu, loaded an existing customized car in the garage and entered
free roam with the HUD, minimap marker and garage-entry prompt. `status` from a
separate process reported `ready`; `stop` closed the game and server, reported
`finished`, and left no owned processes or listener on port 3216. This run used
the garage development server at `1cf3694` and an isolated copy of its account.
No activation dialog appeared; external networking remained connected.

The same revision passed local non-admin Windows fmt, Clippy and all 1,174
workspace tests, including 66 launcher and process-wrapper tests. The CLI
installation-refusal test now uses a Windows executable suffix, so it reaches
the intended hash check on non-elevated developer machines as well as Unix.

A second ordinary-user run on the combined garage/launcher branch reached the
saved garage, free roam and PageDown garage return, then quit normally with
Alt+F4. The launcher stopped its server and supervisor and released port 3216.
Raw protocol recording was off. Default logs showed unsupported LSX, backend,
world message, logic and RPC routes; the closing world summary counted 52
observations, 13 warnings and 39 suppressed repeats. No activation prompt
appeared and external networking remained connected. This run validates the
observed diagnostics and normal cleanup, not the unexercised routes below.

The combined workspace passed fmt, Clippy and 1,342 tests; the optional
network-test build passed Clippy and 193 tests. Unsupported File/Items, unknown
control/decoder routes, overflow, privacy and cancellation use constructed
regression tests rather than malformed traffic sent to the game.

A further ordinary-user Windows check completed standalone `setup` against the
supported installation and created a fresh local account. `setup --json` now
returns one final status object on stdout; progress and child diagnostics go to
stderr and remain visible in an ordinary terminal. Setup also verifies its
preparation children have exited before disarming its session job.

Force-terminating only the supervisor of a ready, routed game session closed its
game and server through the Windows job and released port 3216. Both the frontend
result and separate `status` reported `interrupted`; the frontend returned exit
code 3. This was a startup crash test, not an interruption during a gameplay save.
After moving aside only that stopped test account's `account.json`, `recover`
restored its pointer and a repeated recovery succeeded without changing any
database bytes. Recovery during an active session correctly refused to proceed.
The recovered account subsequently reached the menu and its stock Mustang garage.
Normal Alt+F4 shutdown then finished with exit code 0 and released all owned
processes and port 3216. These corrections passed fmt, Clippy and 1,344 workspace
tests on Windows.
Plain-text `status` also shows recorded process IDs, which may be stale and must
be checked before manually stopping a Unix orphan.

During a later Windows launch on 2026-10-16 using `c8db445`, the owner confirmed
that the EA activation dialog appeared and they completed it. The agent observed
the subsequent `ready` status and normal Press Space splash; `stop` then finished
with exit code 0, all owned processes gone and port 3216 free. This validates
continuation after owner-performed activation on that installation. The agent
missed the dialog itself, and no `activation` state appeared in the launcher log.
Waiting beyond the normal startup deadline, visible activation-state reporting
and cancellation while the dialog is open remain unverified. This does not
establish fresh-install or activation-independent operation.

A subsequent 2026-10-16 run reproduced a path-comparison bug: the joined
activation-helper path contained a forward slash while Windows reported
backslashes. The launcher consequently missed the visible owned dialog and
timed out. [Issue 88](https://github.com/afterhours-crew/afterhours-2015/issues/88)
normalizes that path without changing process/job ownership checks. With the fix,
the launcher reported `activation`, waited while the owner completed the dialog,
then reached `ready` and the splash. Normal Alt+F4 ended with exit code 0 and no
owned game/server processes left. The observed activation wait was about 62
seconds, below the normal startup deadline; prolonged waiting and cancellation
while activation is visible still need a live check. The path regression and
wrong-directory/executable rejection are also covered by constructed tests.

On 2026-10-16 the owner accepted the Rust CLI milestone in
[issue 78](https://github.com/afterhours-crew/afterhours-2015/issues/78) for merge
with these live checks explicitly **untested**, rather than requiring them for
this checkpoint:

- Leaving the activation dialog open beyond the normal startup deadline
  and cancelling while the dialog is open.
  A repeat launch after owner login/logout opened the game directly, so it did
  not exercise this path. Successful continuation after activation is covered above.
- Cold disconnected startup and restart using the Rust CLI. The previous
  PowerShell launcher's disconnected result does not establish this check.
- Linux `setup` and real `serve` using a copied supported installation, including
  readiness and the documented orphan/status behavior after supervisor termination.
  Linux CI and constructed tests do not establish this installed-content check.

Closing the milestone does not mark these checks as passed. The supported
installation and activation prerequisites above still apply.

### Manual disconnected test without administrator access

Build and run `start` successfully while connected first, then quit normally
and back up the stopped data directory. Keep the EA app closed. Disconnect the
machine's external network yourself (unplug Ethernet and disconnect Wi-Fi),
restart Windows while remaining disconnected, and run the same `start` command
from an ordinary terminal. No firewall or service changes are required.
Wait for **Ready to play**, then reach the menu, garage and free roam, return with PageDown,
and quit normally. Start again while still disconnected and verify the same
saved garage. Keep the network disconnected through both complete shutdowns.

Record the two session IDs, any activation dialog or failure, the visible game
results and both final statuses. Repeat with a separate new `--data-dir` to
cover a freshly generated local account without replacing the existing one.
An activation dialog while disconnected is a failed cold-start gate; reconnect
before any owner-performed activation and label subsequent tests accordingly.
Retained installed licenses remain a prerequisite even if this test passes.

The LSX API remains available separately with `--lsx-port 3216` on a configured
server. The [server guide](../crates/server/README.md) describes the underlying
content/state options; `prepare-local --game-dir ... --root ...` creates the
launcher's local policies without starting any services.

## Ownership and extension points

| Component | Owns | Does not own |
| --- | --- | --- |
| [`nfs-lsx-codec`](../crates/lsx-codec/src/lib.rs) | NUL framing, bounded XML, typed envelopes and transport transforms | Account policy, sockets or captured replies |
| [`nfs-lsx`](../crates/lsx/src/session.rs) | One connection's challenge, configuration, authorization code and presence | Global account state, persistence or network access |
| [`nfs-server::lsx`](../crates/server/src/lsx.rs) | Loopback sockets, OS entropy, connection limits, deadlines and cancellation | Game activation, installation changes or public authentication |
| [`nfs-services`](../crates/services/src/authentication.rs) | Backend authentication against the selected local account | Validation of codes by EA or a multiplayer credential authority |

The socket edge takes a profile from the same owned identity used by backend
authentication. A core caller must provide `Config::for_profile(profile)` and a
`Random` implementation; there is no default account. `Fixed` is for deterministic
tests only. The production edge supplies 48 fresh OS-random bytes: 16 for the
challenge and 32 for a connection-local authorization code.

`Launcher::new` returns the session and its opening output. Pass arbitrary input
chunks to `receive`, write each `Output::send` frame in order, and inspect events.
Each emitted frame is already NUL terminated. Close a rejected session; reconnect
with fresh entropy and a new session. The core has no sockets or clock: the edge
enforces timing, write completion and cancellation.

## Wire behavior and local policy

The opening `Challenge` is a clear-text, uncorrelated `Event` from `EALS`.
`ChallengeResponse` must prove the challenge and carry a valid hexadecimal peer
key. `ChallengeAccepted` remains clear text; subsequent requests and responses
use the derived transport transform. A mismatched challenge permanently rejects
that connection. Only request envelopes are dispatched after establishment.

| Request | Implemented behavior |
| --- | --- |
| `GetConfig` | Advertises SDK, profile, login and presence facilities |
| `GetProfile` | Returns index zero for the caller-supplied account/persona |
| `GetSetting`, `GetGameInfo` | Returns explicitly configured local values; absent values remain unsupported |
| `GetInternetConnectedState` | Numeric `1`/`0` for local service availability; performs no Internet probe |
| `GetAuthCode` | Bounded client/scope fields; a stable 64-character hexadecimal value for retries, fresh on reconnect |
| `GetPresence`, `SetPresence` | Current account only; bounded text and typed state; invalid writes cannot mutate state |

Presence is ephemeral and resets on reconnect. It does not write progression or
advertise a multiplayer session. Authorization codes are local compatibility
values. The loopback backend shape-checks them and selects the configured account;
it does not redeem them against an issuer or prove a remote player's identity.
Adding multiplayer requires an explicit authentication and account-selection
design before exposing these services beyond loopback.

Unknown requests generate an unsupported event and no fabricated success.
The edge logs request names and IDs, not authorization codes or payload bodies.
Add new wire forms to the typed codec, put state transitions in the core, and add
constructed tests for ordering, retries, invalid input and account isolation.
Do not insert captured reply templates into the runtime.

## Limits and operation

The edge binds IPv4 loopback, accepts at most four concurrent clients, and closes
excess connections. Slots recover when completed tasks are reaped. `--lsx-port 0`
selects an ephemeral port; the readiness JSON reports its `lsx` address. Omitting
the flag disables LSX and reports `null`. A port collision fails startup; stop
the conflicting listener before retrying.

`--idle-seconds` (default 120, range 1–3600) bounds the whole initial handshake
and completion of each partial frame. An established connection may remain quiet
between complete frames indefinitely; the SDK has no required heartbeat. Writes
have a five-second deadline.
Frames must be smaller than 64 KiB, pending input is bounded to 128 KiB, and XML
has input, nesting and node limits. Server shutdown cancels and joins active
connections, then releases the listener. Do not run a second LSX provider on the
same port or try to solve a bind failure by changing system service settings in
the protocol core.

Startup networking can report an unresolved external endpoint before a resolved
one. The backend accepts the first zero-to-resolved transition only when the
machine identity and internal endpoint remain unchanged. An established external
endpoint cannot be replaced. This is a session-state transition, separate from
LSX connectivity reporting.

## Validation and remaining delivery work

All automated tests use constructed inputs and ephemeral loopback sockets. They
need no game installation, EA service, private fixtures or account profile:

```sh
cargo test -p nfs-lsx-codec -p nfs-lsx --locked
cargo test -p nfs-server lsx --locked
cargo test -p nfs-services --test users_metrics --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The local live validation used `NFS16.exe` SHA-256
`92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df`.
After a Windows restart, the installed game reached menu, garage, free roam and
garage return with EA Desktop absent, its service stopped, and non-loopback
traffic blocked for the game, its activation helper and this server. A restart
of both game and server recovered the same saved garage. The second shutdown
finished after the temporary block expired; its final seconds are outside the
network-isolated claim. Other applications and system DNS remained online. No
external IPv6 route was available to measure a drop.

The guarded validation procedure separately supplied the process-local launch
marker, temporary launcher registration, desktop-shell launch and reversible
in-memory endpoint routing. Those operations are not performed by `--lsx-port`.
The installed activation helper and existing license/cache state were retained.
This establishes launcher/service-free gameplay for that installation and account,
not activation independence or fresh-install acceptance.

The PowerShell launcher that preceded `nfs-afterhours` owned setup, launch,
routing and cleanup using the existing registration without changing it, and
its validation included the owner's cold disconnected test. The Rust launcher
carries the same checks; its completed live checks and remaining gates are described
above. Activation-cache lifetime remains a separate acceptance gate.
The LSX component can be developed and tested independently of installed-game
validation.
