# Local launcher integration

Afterhours implements the local LSX service used by the supported NFS 2015 PC
client. Enable it with `--lsx-port 3216` on an otherwise configured `nfs-server`
invocation. It shares the backend's SQLite account identity and shutdown signal.
See the [server configuration guide](../crates/server/README.md) for the required
content, policy and state inputs. This option starts the IPC listener; a packaged
game launch and routing command is still future work.

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

Reads use `--idle-seconds` (default 120, range 1–3600); writes have a five-second
deadline. The read deadline measures inactivity, not a total handshake duration.
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

The next delivery step is a portable launcher that owns those operations and
their cleanup, with explicit installation/build guards, bounded lifetimes and
recovery after failure. It must avoid permanent registration changes and leave
game files untouched. Validate the complete packaged path after a cold restart,
with external game traffic blocked through final process exit, then separately
test fresh account startup and license/cache lifetime. Preserve existing owner
state throughout those tests. The LSX component can be developed and tested here
independently of that remaining integration.
