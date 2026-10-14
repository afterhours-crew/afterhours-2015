# Run the local game and server

The foreground PowerShell script starts Afterhours and the supported installed game
in one ordinary-user session. It generates its configuration from the installation
and creates a separate local account. It does not need a research repository,
private scripts, copied profile or captured service replies.

**Development preview:** local menu, garage, driving and repeated garage returns
have been tested on the supported installation, including an owner-run cold
disconnected test of this public script. Complete progression remains open. The precise
validation scope and installation limits are listed below.

This is a **source-only distribution**. You must compile `nfs-server.exe` before
running the script; no executable, installer or game content is shipped. All
build and runtime source is in this public Afterhours repository.

## Install the build tools

For the Windows game-and-server path, install these tools first:

| Tool | Required setup |
| --- | --- |
| [Git for Windows](https://git-scm.com/install/windows) | Provides `git` for cloning and updating this repository. |
| [Rust via rustup](https://rustup.rs/) | Use the **x86_64-pc-windows-msvc** host. This checkout's `rust-toolchain.toml` selects Rust **1.98.1**, rustfmt and Clippy; rustup downloads them when first needed. |
| [Visual Studio C++ tools](https://rust-lang.github.io/rustup/installation/windows-msvc.html) | Select **Desktop development with C++**, including the MSVC x64/x86 compiler and a Windows SDK. Visual Studio Build Tools is sufficient; the Visual Studio IDE is optional. |
| [Strawberry Perl](https://strawberryperl.com/) | Put `perl.exe` on your user `PATH`. The vendored OpenSSL build uses it. |
| [NASM](https://www.nasm.us/) | Put `nasm.exe` on your user `PATH` for the OpenSSL assembly build. |
| [PowerShell 7](https://learn.microsoft.com/en-us/powershell/scripting/install/install-powershell-on-windows) | Use **x64 PowerShell 7.5 or later** (`pwsh.exe`), not the built-in Windows PowerShell 5.1. Microsoft's ZIP package is an option for a user-local installation. |

Reopen your terminal after installing tools or changing `PATH`. Installing the
compiler/SDK may require an administrator; building and running Afterhours use an
ordinary user account. The launcher rejects an elevated session. There is no
separate OpenSSL or SQLite service to install: Cargo builds those libraries from
the locked dependencies. No Python, Node.js or private research tools are required.
PowerShell compiles the bundled C# helpers itself; there is no separate launcher
build command or .NET SDK requirement.

Internet access is needed initially to obtain the source, Rust toolchain and Cargo
dependencies. After caching them, Cargo accepts `--offline` for builds. That build
cache is separate from the installed game's activation state described below.

## Requirements

- A legitimately installed, previously activated Need for Speed (2015) PC game.
  Keep the existing installation and license cache. Fresh activation, expired
  caches and running after uninstalling all EA software are not established.
- 64-bit Windows and 64-bit PowerShell 7.5 or later, running without elevation
  from an ordinary terminal, outside an IDE or agent process job.
- Existing 32-bit `HKLM\SOFTWARE\Origin\ClientPath` registration. The tested
  registration names `EADesktop.exe`. `Origin.exe` compatibility is implemented
  but has not been tested live. Other names are refused.
- A built `nfs-server.exe`, following [Build from source](#build-from-source).
  Launching still needs PowerShell, the `launcher/` files and the game installation,
  but does not invoke Git, Cargo, Perl or NASM.
- Close the EA app and any running copy of the game. Local TCP port 3216 must be
  available. Afterhours reports conflicts and leaves the other process alone.

| Installed file | Accepted SHA-256 |
| --- | --- |
| `NFS16.exe` | `92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df` |
| `Core/ActivationUI.exe` | `5901b0ad269e12deecc2bcd2eaa56f8964538a8d6b1a265651d56dbd636daefd` |

Other builds fail before game launch. Do not replace a digest to bypass a failure:
content mappings and the routing adapter also depend on that exact executable.

## Build from source

Open an ordinary, unelevated PowerShell 7 window and clone the public repository:

```powershell
git clone https://github.com/Afterhours-Crew/afterhours-2015.git
Set-Location afterhours-2015
```

Check the tools in that window before building:

```powershell
$PSVersionTable.PSVersion    # 7.5 or later
[Environment]::Is64BitProcess    # True
rustc --version             # 1.98.1, selected by this checkout
rustc -vV                   # host: x86_64-pc-windows-msvc
cargo --version
perl -v
nasm -v
```

Build the release server from the repository root:

```powershell
cargo build --release -p nfs-server --locked
if ($LASTEXITCODE -ne 0) { throw 'Server build failed; read the Cargo error above.' }
./target/release/nfs-server.exe --help
```

The first build also compiles OpenSSL and SQLite and can take several minutes.
The resulting executable is `target/release/nfs-server.exe`. Keep the `launcher/`
directory from the same source revision. If you set `CARGO_TARGET_DIR` or use a
custom Cargo target, locate that build's executable and pass its full path using
`-ServerPath` to `Check`, `Setup` and `Start` below.

If Cargo reports a missing linker, `cl.exe`, `nmake` or Windows SDK library, repair
the C++ workload/SDK installation. For missing `perl` or `nasm`, fix their user
`PATH` and reopen the terminal. For a build that cannot discover an installed
compiler, use Visual Studio's **x64 Native Tools Command Prompt** to run Cargo;
return to an ordinary PowerShell 7 window for game startup. Do not change the
pinned toolchain or dependency versions merely to get past a setup error.

## Start

From the same Afterhours checkout, with the build complete:

```powershell
./launcher/Start-Afterhours.ps1 -GameDirectory 'F:\EA Games\Need for Speed'
```

Use your actual installation path. The default data directory is
`$env:LOCALAPPDATA\Afterhours2015`; `-DataDirectory 'D:\My Afterhours data'` selects
another dedicated local directory. Keep it outside the game installation.
Spaces and Unicode characters are supported. Do not use junctions, symlinks,
network shares or a drive root as the data directory.

The first start generates and validates content, creates an independent identity
and saved database, starts loopback services, and opens the game. Subsequent starts
reuse verified content and the same account. Preparation can take a minute or
more. The script stays in the foreground and prints preparation, server diagnostics
and session status in the same terminal. Wait for its `ready` message before
pressing Space in the game. You can also check from a second PowerShell window:

```powershell
./launcher/Start-Afterhours.ps1 -Action Status
```

Pass the same `-DataDirectory` to every command if you selected a custom location.
`ready` means server listeners and guarded routing are ready; it does not mean the
game has logged in or reached a playable world. Continue through the normal game
menus yourself. The launcher never presses keys or accepts agreements.

If the installed activation helper opens a sign-in window, the script prints
`activation` and waits for you without a startup timeout. It does not read or
submit credentials. This is EA's activation requirement, separate from your local
Afterhours account; local play is not ready while it waits. You can cancel with
`-Action Stop` from another terminal. If activation replaces the original game
process, this session ends with an error; start the script again after activation.
It never adopts a different game process without the original identity guards.

`-PlayerName 'My Driver'` sets the first account's display name. It does not rename
or replace an existing account. New accounts start **after the scripted prologue**;
the official prologue and complete progression are not yet supported. The local
challenge menu uses three fixed preview objectives derived from installed type
metadata. It grants no challenge rewards and does not implement rotating schedules
or challenge completion. No remote product entitlements are granted.

For a custom Cargo target/output location, pass `-ServerPath` with your built
executable. The default is `target/release/nfs-server.exe` inside this checkout.
For server development without opening the game, see
[Run only the server](../crates/server/README.md#run-only-the-server-on-windows).

## Check, setup and stop

```powershell
# Read-only: installation hashes, path safety, registration and port/process conflicts.
./launcher/Start-Afterhours.ps1 -Action Check -GameDirectory 'F:\EA Games\Need for Speed'

# Generate content and account without starting the game or listeners.
./launcher/Start-Afterhours.ps1 -Action Setup -GameDirectory 'F:\EA Games\Need for Speed'

# Close this session's game, then its server.
./launcher/Start-Afterhours.ps1 -Action Stop
```

Quitting normally in the game also shuts down the server. Stop first requests
normal game closure, then terminates only its owned process tree if closure does
not complete within 15 seconds. A single data-directory lock prevents concurrent
supervisors. Global game/process and LSX-port checks reject other active sessions.

The script starts a dedicated PowerShell child in the same console and waits for
it to finish. No shortcuts, desktop automation or background launcher app are
needed. The session process puts itself and its children in a Windows job with
kill-on-close. If the supervisor is forcibly terminated, Windows closes that
owned process tree. If the server dies, the supervisor closes the game. Process
handles and creation identities prevent cleanup from targeting a reused PID.
The memory lease lasts for game lifetime; active play has no experiment timer.
Established idle LSX connections remain open; incomplete handshakes/frames and
writes still have deadlines. Reboot leaves no registration or service change to
undo. SQLite crash recovery runs on the next start; this is not a promise against
hardware failure or an unverified power-loss scenario.

## Data and recovery

Keep `account.json` and `artifacts/account/` together. Stop the session before
backing up the entire data directory, including SQLite journal files if present.
The launcher never resets an existing account or imports an EA save.

`Status` reports `interrupted` when a previous live status has no supervising lock.
Start again after its processes have exited. If setup was interrupted after the
account database was committed but before `account.json` was written, preserve
the directory and recover the small pointer using the account ID from the
successful creation output or the `<id>.sqlite` filename:

```powershell
./launcher/Start-Afterhours.ps1 -Action Recover -AccountId '<32 lowercase hex digits>'
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
activation. It does not implement or supply licenses.

The C# routing adapter changes 13 startup data bytes in the owned game process.
It checks executable identity, live code guards, root thread/TLS/provider shape,
original bytes and the owned server's loopback listener. It verifies writes and
can reverse partial writes without overwriting another writer's data. If the game
has already consumed its startup routing, preparation fails closed. Once the
pinned game process exits, its temporary address space is gone. No executable
instructions, disk patches, debugger attachment or broad process injection API
are exposed by this adapter.

## Diagnostics and troubleshooting

Each launch gets a unique `artifacts/launcher/<session-id>/` containing status and
bounded stdout/stderr logs. Default server recordings contain route metadata and
lengths, not request payloads. Each log sink retains at most 1 MiB of characters
(up to 4 MiB UTF-8); the server recording has a 64 MiB, 100,000-event and 256-stream
budget. It stops recording at the limit and continues serving. These bounds are
per run; old sessions are retained for deliberate user cleanup.

`-RecordProtocol` explicitly enables local raw diagnostic capture. Those files
can contain identities and local authentication inputs. Review/redact them before
sharing; do not publish saved databases, activation files or extracted game data.
There is no upload/export feature that silently sends them anywhere.

| Result | Next step |
| --- | --- |
| Unsupported hash | Use the supported installation; do not weaken guards. |
| Existing game/EA process or occupied port 3216 | Close the conflicting application yourself, then retry. |
| Missing/unrecognized registration | This installation is outside the supported launch path. No automatic repair or admin fallback runs. |
| Already inside a process job | Run from an ordinary PowerShell terminal outside the IDE/agent. No automatic desktop handoff or elevation is attempted. |
| Early script failure | Read the error in the terminal and inspect the printed session-log directory. |
| `activation` | Respond to the installed game's dialog yourself, or cancel with Stop. Launcher-free operation still depends on valid installed activation state. |
| `failed` during preparation | Read that run's preparation log. Preserve existing state. |
| `failed` during routing | Let owned cleanup finish, then retry from a new process. |
| Game exits with a nonzero code | The status records the code; inspect Windows Application events and local logs. A crash is not a successful launch. |
| Script execution blocked by local policy/security software | Follow your system's approved script/signing policy; do not disable protections for Afterhours. |

## Developer validation and current limits

Run the normal three Cargo gates plus these ordinary-user Windows checks:

```powershell
./launcher/Test-Launcher.ps1
./launcher/Test-Session.ps1
```

The first covers memory transactions, partial writes, foreign changes, marker,
path/link rejection, stale status, executable integrity and bounded UTF-8 output.
The second starts disposable PowerShell children and
checks an unelevated worker outside an existing job, paths containing spaces,
Unicode and apostrophes, forced supervisor death and graceful job cleanup with
pinned handles. It
does not start the game or change Windows settings.

The public path has reached the menu with an independently created post-intro
account and recovered that account on subsequent game/server starts. The latest
debug-build run reached its saved garage, drove outside, displayed the own-car
minimap arrow, and completed two garage returns. Both exits displayed Enter Garage;
PageDown worked from the spawn and approach, driving away cleared the prompt,
and approaching restored it. Loading screens had no HUD/minimap/prompt; individual
screenshots do not establish frame-by-frame timing throughout every cinematic.
Normal quit closed the game, server and listener. The actual EA app was absent,
its service was already stopped, and the launcher changed no machine settings.

The standalone release also reached free roam after the owner completed the
installed game's activation dialog. The same original game process continued
after activation. PageDown returned from the garage approach, the next exit
displayed the prompt and player arrow, and PageDown returned again from the
spawn. Normal quit cleaned up the game, server and listener. An immediate
game/server restart required no further sign-in, restored the same account's
Mustang garage and quit normally. The owner reported a delay before the approach
prompt appeared; the later driving regression below addresses that backlog.

After another Windows reboot, the owner reported successful disconnected launch,
driving and garage return twice with the existing account and once with a fresh
local account. Completed session logs corroborate those transitions and normal
supervisor shutdown. Network disconnection was performed and reported by the
owner; no independent network sampler ran during that test. The existing installed
activation cache was retained after earlier online activation. A fresh local
account is independent server data, not a fresh game installation or license.
The owner estimated roughly ten seconds for both the approach prompt and the
response after PageDown. Driving exposed a queue problem: each accepted movement
sample sent chassis and minimap updates as separate frames. They now share a frame
when the existing size limit permits; larger batches retain separate framing.
A subsequent local release test recorded 1,127 replication sends over two minutes
with no remaining backlog. The approach prompt was visible in the first screenshot
about 1.4 seconds after the server detected re-entry, and PageDown began the
transition immediately. These are sampled UI observations, not exact first-visible
latencies. Garage-return animation and loading follow that response; their
durations have not been compared with the original service.
The following exit again showed the prompt and minimap arrow; PageDown at spawn
returned successfully. Normal quit cleaned up the game, server, listener and test
console. The EA app was absent and its service remained stopped.

Rust formatting, Clippy and 1,108 tests pass, along with the Windows launcher
and disposable process-cleanup checks above. The next-day launch initially
required manual activation; the successful release gameplay and immediate
restart do not establish next-day activation-cache acceptance. The later owner-run
cold test establishes the scope described above. Fresh-install/cache-lifetime
acceptance remains open. Earlier research runs do not substitute for public-script checks.

### Manual disconnected test without administrator access

Build and run the script successfully while connected first, then quit normally
and back up the stopped data directory. Keep the EA app closed. Disconnect the
machine's external network yourself (unplug Ethernet and disconnect Wi-Fi),
restart Windows while remaining disconnected, and run the same Start command
from an ordinary PowerShell window. No firewall or service changes are required.
Wait for `ready`, then reach the menu, garage and free roam, return with PageDown,
and quit normally. Start again while still disconnected and verify the same
saved garage. Keep the network disconnected through both complete shutdowns.

Record the two session IDs, any activation dialog or failure, the visible game
results and both final statuses. Repeat with a separate new `-DataDirectory` to
cover a freshly generated local account without replacing the existing one.
An activation dialog while disconnected is a failed cold-start gate; reconnect
before any owner-performed activation and label subsequent tests accordingly.
Retained installed licenses remain a prerequisite even if this test passes.

The LSX API remains available separately with `--lsx-port 3216` on a configured
server. The [server guide](../crates/server/README.md) describes the underlying
content/state options; `prepare-local --game-dir ... --root ...` creates the
public launcher's local policies without starting any services.

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

The public launcher described above now owns setup, launch, routing and cleanup,
using the existing registration without changing it. Its validation is reported
separately from that earlier procedure, including the owner's cold disconnected
test. Activation-cache lifetime remains a separate acceptance gate.
The LSX component can be developed and tested independently of installed-game
validation.
