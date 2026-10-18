# Afterhours 2015

[![CI](https://github.com/afterhours-crew/afterhours-2015/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/afterhours-crew/afterhours-2015/actions/workflows/ci.yml)

Server implementation for Need for Speed (2015), with offline play and custom
servers as the goals. This is the canonical home for runtime code and its
first-party dependencies.

Components arrive incrementally once their behavior is understood and tested.
The standalone server and the [`nfs-afterhours` launcher](docs/LOCAL-LAUNCHER.md)
build here. The launcher prepares content from a supported installation,
creates an independent local account, runs the server on any platform and on
Windows starts and routes the game. Its current validation scope and installation
requirements are documented in that guide. Tested gameplay and the prioritized
remaining systems are tracked in the [gameplay roadmap issue][roadmap] and its
per-system tracking issues. The tested garage customization scope and local
test-cash/REP commands are covered in [garage upgrades and customization](docs/GARAGE-UPGRADES.md).
[Car ownership and garage management](docs/CAR-OWNERSHIP.md) describes the current
implementation and its acceptance status.
The workspace owns these implemented components:

| Component | Responsibility |
| --- | --- |
| [nfs-server](crates/server/README.md) | Runnable local server, owned control integration, world listeners and deployment configuration |
| [nfs-fire2](crates/fire2/README.md) | Bounded stream framing and incremental decoding |
| [nfs-heat2](crates/heat2/README.md) | Bounded tagged fields, containers and canonical encoding |
| [nfs-protocol](crates/protocol/README.md) | Typed backend payloads and bit-oriented world codecs |
| [nfs-world-core](crates/world-core/README.md) | Transport, application state, File transfers, item models and startup registration codecs |
| [nfs-world](crates/world/README.md) | Owned world sessions, typed replication, participant and vehicle state |
| [nfs-lsx](crates/lsx/README.md) | Connection-owned launcher handshake, profile and local authorization codes |
| [nfs-lsx-codec](crates/lsx-codec/README.md) | Launcher framing, XML, envelopes and transform codecs |
| [nfs-services](crates/services/README.md) | Owned startup groups, current account stats and reputation, client-state/telemetry handling, local recommendations/wrap listings, inventory, persistent-table views and content loaders |
| [nfs-storage](crates/storage/README.md) | Account-owned inventory, garage slots and tables, with memory and SQLite adapters |
| [nfs-frostbite](crates/frostbite/README.md) | Bounded, read-only reader for an installed game's content containers |
| [nfs-content](crates/content/README.md) | Builds and caches deployment content from the player's own installation |
| [nfs-afterhours](crates/launcher/README.md) | Command-line launcher: preflight, content and account preparation, server supervision, and on Windows game launch and routing |
| [nfs-afterhours-sys](crates/launcher-sys) | The one crate with `unsafe`: Win32 and libc calls behind safe, bounded wrappers |

Tests use constructed inputs, standard cryptographic vectors and temporary
databases. Game files and service access are unnecessary. See the
[contribution and promotion process](CONTRIBUTING.md).

## Build and run

Afterhours is distributed as source: **no server executable or installer is
included**. Start with the [setup guide](docs/LOCAL-LAUNCHER.md), which lists
the build tools, supported game installation and activation requirements.
Once those prerequisites are installed, open an ordinary, unelevated terminal:

```sh
git clone https://github.com/Afterhours-Crew/afterhours-2015.git
cd afterhours-2015
cargo build --release -p nfs-server -p nfs-afterhours --locked
./target/release/nfs-afterhours start --game-dir "F:\EA Games\Need for Speed"
```

Replace the game path with your installation directory. The build produces
`target/release/nfs-server` and `target/release/nfs-afterhours` (`.exe` on
Windows). The launcher prepares local content and an account, starts both the
server and game, and keeps logs visible in the terminal. Wait for `ready`
before pressing Space. Normal game exit also stops the server. On Linux or
macOS, `nfs-afterhours serve` runs the server without a game. Running
Afterhours does not require administrator access; installing third-party build
tools may. No private checkout, copied profile or database service is needed.

For custom build output paths, saved data, setup checks, exit codes and
troubleshooting, see the [running instructions](docs/LOCAL-LAUNCHER.md#build-from-source)
and the [launcher crate](crates/launcher/README.md).
Developers can also [run just the server](crates/server/README.md#run-only-the-server).
For menu automation, [change the game's Back key to F9 without a mod](docs/MENU-KEYBINDING.md).

## Build and test

Install Rust through rustup and a native C toolchain (MSVC Build Tools on Windows,
or a C compiler on Linux), and Perl for the vendored OpenSSL build (for example,
Strawberry Perl on Windows, with `perl` on PATH). Windows also needs NASM on PATH
for OpenSSL assembly. `rust-toolchain.toml` selects Rust 1.98.1, rustfmt and
Clippy. SQLite is built from the bundled dependency; no database service is needed.
Cargo downloads locked dependencies on the first build.
The [Windows prerequisite checklist](docs/LOCAL-LAUNCHER.md#install-the-build-tools)
links the installers and names the required components. The commands below build
and check the entire workspace; they do not start the game.

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Once the toolchain and dependencies are cached, add `--offline` to Cargo build,
Clippy and test commands. No private repository, account profile or recording is
needed. Passing these tests establishes the covered component behavior, not
offline play or launcher independence. New local accounts start after the
unsupported scripted prologue; story progression and multi-car garage management
remain unfinished. The [garage customization guide](docs/GARAGE-UPGRADES.md)
records supported purchases, tuning, appearance, wraps and resets. A guarded
local launch procedure
has reached free roam after a Windows restart without the EA app or service and
with external game traffic blocked. The launcher supplies setup, process
ownership and temporary routing without administrator privileges; its
validation status is reported in the [running instructions](docs/LOCAL-LAUNCHER.md).
Existing installation registration and activation-cache requirements remain.

The world attribute service consumes a committed readiness permit, validates the
current group-to-world association and builds a typed notification. The association
becomes visible only after both frames write; retries ACK without republishing.
Bounds, failed writes and foreign policies close the session. This supports the
single current-world association, not arbitrary attribute edits.

The local self-mesh service generates the validation notification and correlated
ACK from the accepted world binding. It reports publication only after the pair
writes, closes on failed writes or premature retries, and bounds repeat requests.
This does not replace the independent host reliable-synchronization gate.

The initial matchmaking diagnostic uses bounded local search policy and accepted
session IDs. Its notification publishes only after a complete write; retries do
not publish twice. It does not allocate a world or assert a successful match.

The world setup service constructs the complete typed setup from bounded deployment
policy, accepted player state, fresh allocation and injected match measurements.
It retains no recorded reply. Publication waits for the complete write; failed
writes and premature retries invalidate the world.

Fresh matchmaking/world allocations use injected raw entropy, local ID domains
and a caller-owned loopback endpoint. Matchmaking acknowledgements bind an
admitted current group/player context, commit only after complete writes and
bound retries. Typed admission validates the complete supported request form
against explicit rule policy and the initialized group's current user and network
binding. Configuration contains named rules and local limits, never serialized
requests. This covers a single local member; broader matchmaking remains work.

Settings use typed per-key changes and account-owned SQLite documents. The edge
refreshes state before each request, commits changes before success replies and
keeps committed state when a reply is lost. Bounded compare-and-swap retries merge
concurrent connections without replacing unrelated keys. Settings share the
versioned inventory account database with identity, entitlements, kickback and
speedwall state. Validated imports seed only absent domains; existing database
values win. The previous standalone settings file is read for migration and
left intact. Filesystem and SQLite work belongs on blocking workers.

Static control catalogs use named deployment definitions for key scopes, kill
switches and SpeedList types, plus an explicit disabled limited-feature policy.
Typed replies use the current request and authenticated persona. Catalogs retain
no request/reply trees, captured frames or account identity.

User lookup binds initial identity to the latest generated session data. Headset
updates change only that session; external lookup uses explicit caller-owned
directory results. The single-account helper is valid only for a directory with
one local account. Menu news implements a bounded unavailable-feed policy for
the supported locale. Neither service retains captured replies.

Local social queries encode an explicit eligible-player snapshot and known-empty
friend recommendations/recent-player history. Callers supply and refresh current
directory state; an unknown or nonempty history never becomes an empty success.

Entitlement reads use a bounded versioned account-state record with named grants
and supported group scopes. Current authenticated account/persona checks precede
query selection, and stable grant IDs survive reopening. General search and grant
mutations remain unsupported; existing state is never rewritten by a read.

## License

`nfs-server-support` provides bounded HTTP/XML discovery and loopback QoS
listeners, including an ephemeral TLS identity. Blocking adapters belong on
blocking workers. `nfs-services::user_session` owns the ordered network and
latency updates used to initialize each authenticated session. Their portable
tests construct requests and use local ephemeral sockets without recordings.

This repository is licensed under the Mozilla Public License, version 2.0. See
[LICENSE](LICENSE). Each source file carries the MPL notice, so modified copies
of those files must stay under the MPL when distributed, while new files that
only use these crates may carry another license.

[roadmap]: https://github.com/afterhours-crew/afterhours-2015/issues/62
