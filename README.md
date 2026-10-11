# Afterhours 2015

Server implementation for Need for Speed (2015), with offline play and custom
servers as the goals. This is the canonical home for runtime code and its
first-party dependencies.

Components arrive incrementally once their behavior is understood and tested.
The standalone server now builds here; operating a compatible garage requires
separately supplied content and account state. The workspace owns
these implemented components:

| Component | Responsibility |
| --- | --- |
| [nfs-server](crates/server/README.md) | Runnable local server, owned control integration, world listeners and deployment configuration |
| [nfs-fire2](crates/fire2/README.md) | Bounded stream framing and incremental decoding |
| [nfs-heat2](crates/heat2/README.md) | Bounded tagged fields, containers and canonical encoding |
| [nfs-protocol](crates/protocol/README.md) | Typed backend payloads and bit-oriented world codecs |
| [nfs-world-core](crates/world-core/README.md) | Transport, application state, File transfers, item models and startup registration codecs |
| [nfs-world](crates/world/README.md) | Owned world sessions, typed replication, participant and vehicle state |
| [nfs-lsx-codec](crates/lsx-codec/README.md) | Launcher framing, XML, envelopes and transform codecs |
| [nfs-services](crates/services/README.md) | Owned startup groups, current account stats and reputation, client-state/telemetry handling, local recommendations/wrap listings, inventory, persistent-table views and content loaders |
| [nfs-storage](crates/storage/README.md) | Account-owned inventory, garage slots and tables, with memory and SQLite adapters |

Tests use constructed inputs, standard cryptographic vectors and temporary
databases. Game files and service access are unnecessary. See the
[contribution and promotion process](CONTRIBUTING.md).

## Build and test

Install Rust through rustup and a native C toolchain (MSVC Build Tools on Windows,
or a C compiler on Linux). `rust-toolchain.toml` selects Rust 1.98.1, rustfmt and
Clippy. SQLite is built from the bundled dependency; no database service is needed.
Cargo downloads locked dependencies on the first build.

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Once the toolchain and dependencies are cached, add `--offline` to Cargo build,
Clippy and test commands. No private repository, account profile or recording is
needed. Passing these tests establishes the covered component behavior, not
offline play or launcher independence. The account creation command generates
independent local identities and state; client startup with that state, complete
garage mutations and launcher independence remain unfinished.

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
