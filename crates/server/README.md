# nfs-server

Owned local control services and world networking for NFS 2015. This crate
contains the runnable server, a sans-IO control session and the Tokio socket
edge. All first-party dependencies are in this workspace. It has no dependency
on a recorder, private repository, capture manifest or captured reply store.

The server generates replies from deployment policy, local identity, account
state and static content. This is an incremental implementation: unsupported
routes are reported rather than answered with invented success. A successful
start or automated test does not establish complete gameplay or launcher
independence. The repository does not distribute game content or an account
profile; running a compatible garage requires the operator's separately
prepared content and state. The local account creation command is available;
the complete prologue and several garage mutations are still incomplete.

## Build and start

Use the toolchain selected by the workspace. The vendored OpenSSL build needs a
C compiler and Perl; Windows also needs MSVC Build Tools and NASM. Run from this
checkout:

```sh
cargo build -p nfs-server --locked
cargo run -p nfs-server --locked -- --help
```

The executable is `target/debug/nfs-server` (`.exe` on Windows). It can be copied
to a deployment directory. Its working directory need not contain this source
tree. It listens on IPv4 loopback only. No EA authentication, telemetry, DNS
lookup or hosted database is used by the server. Client activation and launching
remain separate concerns.

Create a deployment directory with `artifacts/`, `artifacts/runs/` and a private
configuration directory. Pass `--root <deployment-directory>` and
`--output <absolute-path-to-artifacts/runs/new-run>`. The output parent must
already exist; the final run directory must not. Configuration paths are
relative to `--root`, or may be absolute. `--state-directory` must be a relative
path under `artifacts/`, for example `artifacts/state`.

`--local-account` selects a nonzero 32-digit hexadecimal storage identifier.
The account database holds identity, entitlement grants, kickback, speedwall,
settings, inventory, garage slots and progression tables. Existing state is durable and
must be preserved across restarts. Initial inventory content is applied only
through the account initialization rules; changing content does not reset an
existing account.

Stop older servers and back up the state directory before upgrading. Schema v4
adds the service-state tables without replacing inventory, garage or progression.
For the first upgrade, the existing `--local-identity`, `--entitlement-state`,
`--kickback-state`, `--speedwall-state` and `--user-settings` options supply
validated imports for absent domains. The identity and grants must match the
selected storage account. All candidate domains validate before any are imported.
Legacy settings SQLite takes precedence over legacy settings JSON and the seed;
its revision survives migration and the old file is left intact.

On subsequent starts these five import options may be omitted. Database values
take precedence even when an import path is provided, changed or no longer exists.
Missing or malformed database state fails startup rather than creating guessed
defaults.

## Create a local account

The one-shot command uses OS randomness for a new storage key and distinct local
persona/account IDs. It opens no listeners and contacts no external services:

```sh
nfs-server create-account --name "New Driver" --state-directory artifacts/new-account --account-policy config/account-policy.json --item-content config/items.json --persistent-content config/tables.json
```

Instead of `--item-content` and `--persistent-content`, pass `--game-dir
<installation>` (optionally `--content-cache <directory>`, default
`artifacts/content`) to build both from the player's game files; see
[Game content](#game-content).

`--progression post-intro` saves the account past the official prologue: every
intro objective is stored completed and the main objectives it activates are
stored active, so the client plays the open world normally and enters the
garage on its prompt instead of automatically, as the prologue's "drive to the
garage" step does. With explicit content files it needs `--progression-content
<file>`; with `--game-dir` the generated content is used. The default,
`--progression fresh`, keeps the asset-default tables, i.e. the start of the
prologue, whose scripted events this server does not run.

All paths follow the current directory. The parent of `--state-directory` must
exist and the final directory must be new. Names are nonempty UTF-8, at most 32
bytes, with no control characters. The JSON result contains `created: true` and
the `local_account` key to use with the ordinary server command. Preserve this
directory; a repeated creation command refuses to replace it.

The account policy has exactly `format: "nfs-fresh-account-policy"`, `version: 1`,
`build_sha256`, `screenshot_count_max`, and `entitlements`. The latter contains
`scopes` and `grants`, using the [entitlement state schema](../services/src/entitlements.rs)
with `id` and `persona_id` omitted from each grant. Grant IDs are newly allocated;
these supported grants are account-wide, with persona zero. Dates, status and
counts are explicit local license policy. No base-game or DLC entitlement list
is bundled or inferred from an existing profile.

Creation validates all content in memory, initializes inventory through the Items
recipe and progression through typed asset defaults, then commits them together
with identity, grants, empty settings, zero screenshot counters, no winner and
empty speedwall rows in one SQLite transaction. Existing stores are ineligible.
An allocated directory can remain after an I/O failure for diagnosis. Initial
vehicle choice follows the configured recipe and existing local garage policy.

This provides independent local state, not the official prologue award flow.
Empty menu state and first-login behavior still require client validation. A new
account's empty state is answered without invented values: a license declaration
naming no license receives the observed empty acknowledgement, a settings read of
an absent key answers `UTIL_USS_RECORD_NOT_FOUND`, and a missing kickback winner
answers `KICKBACK_ERR_NOT_FOUND` (codes from the client's error-name tables).
A missing speedwall row remains unsupported.

Leaving the current world (`4/22 leaveGameByGroup`) answers an empty
acknowledgement and a player-removed notification (`PLAYER_LEFT`) for the current
G2 and persona; disconnected reports for the current G1 self mesh and G2 host mesh
receive empty acknowledgements. Departure state commits after the write. It does
not re-enter, create or destroy a group.

## Configuration contract

The CLI requires all control families below. Missing or invalid policy fails
before the readiness announcement. JSON loaders reject unsupported versions,
unknown fields and out-of-bounds collections. Build-specific content checks
the supported image hash exposed by `nfs_services::SUPPORTED_BUILD_SHA256`.
The linked loaders define the exact versioned field schemas.

| Options | Input and schema |
| --- | --- |
| `--bootstrap-config` | Local bootstrap values; endpoints are bound from listeners ([loader](../services/src/bootstrap/content.rs)) |
| `--auth-config` | Authentication policy; identity comes from SQLite ([authentication](../services/src/authentication.rs)) |
| `--local-identity` | Optional one-time account/persona/name import ([account state](../services/src/account_state.rs)) |
| `--group-policy` | Group configuration ([group](../services/src/group.rs)) |
| `--matchmaking-admission`, `--matchmaking-policy` | Admission and status policies ([admission](../services/src/matchmaking.rs), [status](../services/src/matchmaking_status.rs)) |
| `--world-policy` | Allocation/setup policy ([world setup](../services/src/world_setup.rs)) |
| `--control-catalogs` | Typed static menu/catalog content ([catalogs](../services/src/control_catalogs.rs)) |
| `--entitlement-state`, `--kickback-state`, `--speedwall-state` | Optional one-time imports ([entitlements](../services/src/entitlements.rs), [kickback](../services/src/kickback.rs), [speedwall](../services/src/speedwall.rs)) |
| `--item-licenses` | Static item license definitions ([licenses](../services/src/item_licenses.rs)) |
| `--user-settings` | Optional initial settings import; durable SQLite values take precedence ([settings](../services/src/user_settings.rs)) |
| `--stat-definitions`, `--challenge-content` | Definitions used with current account views ([stats](../services/src/stats.rs), [challenges](../services/src/challenges.rs)) |
| `--owned-menu-awards`, `--owned-local-social` | Enable current award reads and the bounded single-account social model |

## Game content

Prepare the supported files independently, before configuring a server or account:

```sh
nfs-server prepare-content --game-dir "F:/EA Games/Need for Speed" --root data
```

This opens no listeners and prints JSON with `prepared`, the `install_content`
cache identity and a `files` map of absolute paths. `--content-cache` selects a
different cache; both it and a relative game path follow `--root` (default `.`).
Only the installation-derived files listed in [nfs-content](../content/README.md)
and waiting/exit sequence definitions are prepared. Garage logic, challenges and
service policy are still supplied separately. Account state is created separately.

`--game-dir <installation>` replaces `--item-content`, `--persistent-content` and
`--world-mac-template` (passing both forms is an error). On the first start the
server reads the installation with [nfs-content](../content/README.md) before it
binds any listener: it checks the supported executable, builds the item
definitions, persistent-table schemas and world MAC template, validates them
through their loaders and caches them under `--content-cache` (default
`artifacts/content`, relative to `--root`). Later starts verify and reuse that
entry; a patched or repaired installation produces a new one. The readiness
line reports the entry as `install_content` (`fingerprint`, `built`,
`directory`). It also supplies `--stat-definitions` when that option is absent:
progression and reputation Stats groups use the installed column names and
defaults. An explicit Stats catalog overrides the generated display policy.
It also supplies `--item-licenses` and `--control-catalogs` when omitted.
Generated licenses recognize duplicate-free subsets of installed license names
without awarding items; they contain no account grants. Generated menu catalogs
derive Stats scopes from the installed game modes and use local AllDrive policy:
time-limited features and recent-player listing are disabled, and no SpeedList
types are offered. Explicit catalogs retain their configured policies.

It also supplies `--reputation-thresholds`, a versioned threshold document built
from the installed reputation calculator. The garage-logic document can omit its
`reputation_thresholds` array; this file then supplies the same thresholds to the
garage, Stats and awards models. An embedded array remains explicit operator
policy and takes precedence (an invalid embedded array still fails startup).
Without `--game-dir`, pass `--reputation-thresholds` explicitly to use the prepared
file. Its schema is `format: "nfs-reputation-thresholds"`, `version: 1`,
`build_sha256`, and `thresholds` (strictly increasing signed integers starting at
zero, at most 256). No account scores are included.
An unsupported executable or unreadable installation stops startup.
No game data is distributed with the server, and other content is still
supplied through the options below.

For world/garage operation also supply:

| Options | Input and schema |
| --- | --- |
| `--world-content` | Version 4 or 5 registrations, scene profiles, launcher definitions and explicit scene roles ([loader](src/content.rs), [roles](src/scene_roles.rs)) |
| `--world-mac-template` | Separate 64-byte MAC parameter input, checked by hash; no bytes are bundled ([validator](src/world_handshake.rs)); generated by `--game-dir` |
| `--item-content`, `--state-directory`, `--local-account` | Item catalog plus durable account state; required together ([item content](../services/src/item_content.rs)) |
| `--persistent-content` | Persistent-table definitions ([persistent](../services/src/persistent.rs)); generated by `--game-dir` |
| `--game-dir`, `--content-cache` | Installation to build item, table and template content from, and its cache ([install content](src/install_content.rs)) |
| `--progression-content` | Progression restoration and entity construction; defaults to installed content with `--game-dir`, including channel bindings and generated bundle IDs ([progression](src/progression.rs)) |
| `--vehicle-content`, `--garage-layout` | Vehicle assets and parking layout; both default to installed content with `--game-dir` ([vehicles](src/vehicle_content.rs), [layout](src/vehicle_content/layout.rs)) |
| `--vehicle-parts` | Part definitions for version-2 vehicle content; defaults to installed content with `--game-dir`. Explicit profiles, parts and layouts retain precedence |
| `--sequence-content`, `--garage-logic` | Sequence definitions default to installed content with `--game-dir`; explicit definitions override them. Garage logic remains separately supplied ([sequences](src/sequence_content.rs), [garage](src/garage_logic.rs)) |

Sequence content version 2 omits `arrive.spline.base_value`: the six-bit goal
allocation counter belongs to the current world and wraps after 63. It advances
only when a goal and its Actor assignment commit together. Version 1 remains
readable; its legacy `base_value` is range-checked and ignored. The current shape
of `arrive.spline` is exactly `asset`, `catalog` and `flag`; runtime counters and
RPC serials are never content fields.

World content versions 1–3 remain readable by offline inspection APIs, but the
runtime requires version 4 or 5 with `roles`. This object names `level`, `gameplay`,
`startup`, `garage`, `progression`, ten ordered `traffic` scene keys, and
`customization_timer`/`streaming_gate` assets (each has `bundle`, `type_id`,
`local_index`). Scene keys must be distinct, greater than one and present in the
scene profiles; root key one is reserved. The level must match the level
registration. These inputs are static roles, never previously allocated ghost
IDs. No default game asset table is embedded in the server. An optional
`spawn_points` key names the level's SpawnPoints scene.

The optional `roles.free_roam` catalog supplies `volumes`, `presence`, `streamed`
and `location` bindings. Each binding contains a static `scene_key` and one-based
`serializer` index. Only unconditional world-volume subscriptions belong in this
catalog; race/event filters require progression eligibility. The loader validates
all sites against registered scene profiles before startup. Activation follows
the owned FreeRoam lifecycle after completion creations and binding refreshes;
without this catalog the additional subscriptions remain unsupported. Installed
content preparation does not yet derive this optional catalog. Owned participants
already in FreeRoam may report their location before the activation queue drains;
this records readiness without bypassing subscription ordering.

Garage logic may supply event-only `world_ready`, `world_spawn` and `free_roam`
batches. They run once at the first new-car readiness report, release of its spawn
assignment, and completion of the queued world handoff, respectively. These
batches use the existing `events` format and may not create/delete objects or
change properties. They complement `exit`, `arrive` and `arrived` presentation
batches. Event targets resolve to current per-participant objects; queue pressure
retains the entire phase for retry. Installed preparation does not yet derive
these optional event phases. Live driving acceptance remains open.

Garage return requires the installed entry sequence and all three garage logic
phases: `return_requested`, `returning`, and `returned`. The entry definition
includes both cinematic channels, its spline and a validated server timer.
The server prepares a replacement garage car, completes the cinematic, detaches
the driven car, and re-arms garage loading for subsequent trips. Missing return
presentation keeps the request unsupported. Portable repeated-cycle tests cover
this path; game acceptance is recorded separately.

At garage exit the server answers the leave request with the participant state
chain and, when the garage layout has a `world_spawn` pose, replaces the garage
car with a driveable world car in the same frame. When the client reports the
new car, the server assigns a spawn point (a per-session id). It answers spawn
requests and releases with the scene's occupied flag. Once a participant has
joined, the server polls the level root about every 1.1 s, and the client's
answers are recorded. Configured arrival sequences, vehicle bindings and world
membership then complete the handoff; this does not yet establish drivable play.

`--redirector-port 0` chooses an ephemeral port (the default).
`--idle-seconds` accepts 1–3600, and `--qos-seconds` accepts 1–86400.
`--stop-file <path>` requests graceful shutdown when that path appears; Ctrl+C
also stops the server. Relative stop paths follow the process working directory.
Legacy `--owned-only`, `--owned-world-connection`, `--owned-world-readiness` and
`--owned-world-attributes` switches remain accepted; the public runtime always
uses owned control models. `--manifest`, `--control-content` and runtime replay
are unsupported.

The first stdout JSON line with `ready: true` includes the actual listener
addresses and configured capabilities. Logs go to stderr. Each run writes
bounded-queue diagnostic recordings below its output directory. Recordings can
contain account data and client authentication inputs: keep them private.
Structured status logs contain route/error summaries, not request bodies.

## Integration and tests

`Deployment` validates local policies and builds fresh `SessionProfiles` using
injected time and a `SeedSource`. `ControlSession` returns reply batches and
advances publication only on `committed()`; callers invoke `write_failed()` if
delivery fails. The socket edge enforces this ordering, stages settings
transactions before acknowledgements, and gates world readiness on the current
transport. The runtime uses OS randomness; deterministic seeds are for tests.

World listeners allocate per-session participants and ghosts, use typed
replication, and obtain inventory/table snapshots from account storage. Scene
roles are validated before allocation. Queues, frames, concurrent connections,
deadlines and blocking workers are bounded. First-party Rust forbids unsafe code.

Run the workspace fmt, Clippy and test gates from the repository README.
Portable tests construct their own data and include authentication ordering,
different identities, failed writes, persistence, partial/combined frames,
timeouts, UDP transport and graceful shutdown. The bootstrap fixture under
`tests/support` contains constructed placeholders; it is not a deployable game
configuration. Private capture comparisons remain outside this repository.

## Installed scene profiles

--game-dir prepares scene-catalog.json and supplies --scene-catalog unless that
option is explicitly set. World content version 5 has the same fields as version
4, but scene_profiles and launchers may be omitted. Missing fields come from the
installed catalog, whose level must match the world configuration. Explicit
fields remain authoritative and are validated; malformed fields never fall back.
Version 4 and inspection formats 1–3 retain their existing behavior.

The catalog schema is format: nfs-scene-catalog, version: 1, build_sha256, level,
and scenes. Each scene has content_key, root, serializers (stable kind names)
and launchers (serializer indexes). Root key 1 is reserved. It contains static
wire shapes, not instance state.

`--game-dir` also supplies generated `--world-content` unless explicitly set.
Its startup registration order comes from the installed level and its role asset
indexes from the installed registries. Existing vehicle, garage, sequence and
progression configuration must use the same generated level IDs. To retain an
older deployment's order, pass its `--world-content` explicitly. Those other
inputs and service policy are still required for a complete deployment.

`--game-dir` supplies a generated `--garage-layout` for the five main parking
slots, their inventory bindings and the installed exit_teleport destination. The
initial world_spawn pose remains separate. Deployments using that pose should keep
their explicit layout while world-exit content generation is completed. Explicit
layout files take precedence; no existing file is rewritten.

`--game-dir` also supplies `vehicle-parts.json` unless `--vehicle-parts` is
explicitly set. To use it, vehicle content version 2 has exactly `format`,
`version`, `build_sha256` and `vehicles`; its vehicle rows keep the version-1
schema, and the embedded `parts` section is removed. Version 2 requires a parts
file and validates both inputs against the same item catalog. Vehicle content
version 1 continues to use its embedded parts, including when `--game-dir` is
present. Preparation does not yet generate vehicle entity profiles.
