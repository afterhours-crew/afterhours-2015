# nfs-content

Builds deployment content from a player's own game installation, once, and
caches it. The server and a launcher call the same library; it distributes no
game data.

```rust
let prepared = nfs_content::prepare(game_root, cache_root, &nfs_content::Options::default())?;
let items = prepared.path(nfs_content::Kind::ItemContent);
```

`prepare` is synchronous; async callers run it on a blocking worker. It:

1. Checks that `NFS16.exe` is the supported build (SHA-256) and fingerprints
   the executable, both layouts, both catalogues and every superbundle TOC,
   plus consumed bundle-manifest and EBX record ranges (including delta bases).
   Warm starts rehash these ranges; unrelated texture/audio records are skipped.
   Manifest inputs identify whole-file digests or domain-separated range digests.
   The fingerprint also covers the selected build profile and construction rules.
2. Returns `<cache>/<fingerprint>/` when its `manifest.json` names the same
   fingerprint and builder revision and every output digest matches.
3. Otherwise reads the installation with `nfs-frostbite`, builds every output
   in memory, validates each through the loader that consumes it, writes a
   temporary directory and publishes it with one rename. An invalid entry is
   replaced only when it holds nothing but entry files. A patched or repaired
   installation produces a new fingerprint and therefore a new entry.

| Kind | File | Source |
| --- | --- | --- |
| `ItemContent` | `item-content.json` | `Items/GameItemSystem`: every definition, its prices, quantity, ownership level, flags, sub/additional items and derived default, plus the starting inventory |
| `PersistentContent` | `persistent-content.json` | `PersistentTableAsset` columns, types and defaults for the tables `nfs_services::persistent` requires |
| `StatDefinitions` | `stat-definitions.json` | Installed columns and integer defaults of `ProgressionStats` and `RepValuesTable`, with the local service's group selection and display policy |
| `ItemLicenses` | `item-licenses.json` | License names from the client's active Win32 `EntitlementQuery` assets; version 2 recognizes subsets without granting items |
| `ControlCatalogs` | `control-catalogs.json` | `SpeedWallAsset` categories and their referenced `GameModeId.IdHash` values, plus local AllDrive menu policy |
| `ReputationThresholds` | `reputation-thresholds.json` | `IntScoreToIntLevelAsset.LevelThresholds`, following the reputation calculator's asset reference |
| `SceneCatalog` | `scene-catalog.json` | Ordered scene composition, class construction rules and installed vehicle bundles; no instance state |
| `WorldContent` | `world-content.json` | Installed startup names, parent relationships, traffic slot order and registry-resolved roles, with authored AllDrive host policy |
| `GarageLayout` | `garage-layout.json` | Main garage slot links, locator transforms and persistent inventory-slot bindings, with graph-derived serializer positions |
| `VehicleParts` | `vehicle-parts.json` | Item-system customization, tuning assets, NOS variants, wheel geometry, wrap/plate identifiers and mesh GUID alternatives |
| `VehicleProfiles` | `vehicle-profiles.json` | Player vehicle blueprints reached through the item system: component profiles, mesh-link order, bus flags, registry identities, physics, appearance and health defaults |
| `WorldMacTemplate` | `world-mac-template.bin` | 64 bytes of the executable image at the build's known address, checked by digest |

Derived defaults are the stored 32-bit patterns of each class's same-named
fields (floats as their bits), three words per colour/material vector and
eight NUL-padded bytes of licence-plate text. Timed discounts have no static
default. Item, table and MAC outputs retain their existing formats. Stats and
menu outputs use the service loaders; license vocabulary uses the version-2
subset policy described below.

The default menu catalog disables time-limited features and recent-player
listing, and offers no SpeedList types. SpeedList offerings belong to operator
policy; an explicit server catalog overrides this default. Stats scopes use
sorted unique mode hashes, merging adjacent values into inclusive ranges.
License vocabulary follows only the active client entitlement service's Win32
queries and deduplicates names; it contains no ownership claims or player grants.

Reputation thresholds must be signed integers, start at zero and increase strictly,
with at most 256 levels. The generated versioned document contains no account
scores. It can supply thresholds omitted from the server's garage-logic document.

## Build profiles

Identifiers that belong to one executable build and cannot be read from the
installed data files live in [`builds/`](builds/README.md), one JSON file per
build: the executable digest, the MAC template's address and digest, and the
runtime class IDs of Blueprint classes, and server scene construction rules. `BuildIdentity::supported()` loads the
profile of the build the loaders accept; adding a build means adding its file.

## Asset identities

`assets::AssetIndex` derives what the client uses to name assets: a bundle's
scene content key (djb2-xor of its lower-cased path), its asset catalog
(Blueprint objects in `<bundle>_networkregistry_win32`, counted by class ID)
and asset references (class ID and position in registry order). A Blueprint
class without a profiled ID is an error, never a guess. These are inputs for
world, garage, vehicle and progression content. Progression construction
definitions are still supplied separately.

## Vehicle parts

The version-1 `nfs-vehicle-parts` document holds five definition catalogs, validated
against the item GUID/class catalog and the same typed models the server consumes.
The builder follows every item-system definition, preserving ordered rim sizes,
tire alternatives and mesh GUIDs. Persistent transmission values follow the item's
tuning asset. Dynamic wraps use the static livery index and all 64 bits of the
asset hash; account-selected wraps and tuning remain account state.

Unknown classes, missing or mistyped required fields, unresolved transmission
assets, duplicate definitions, nonfinite geometry and model bounds fail preparation.
The catalogs have at most 16,384 definitions, individual alternatives at most 256,
and the document at most 16 MiB. The installed item system, rather than a list of
known cars or parts, determines membership.

## Vehicle profiles

The builder follows each player vehicle item's reference to its blueprint bundle.
Traffic vehicles use a separate constructor and are outside this catalog. Entity
serializers follow component preorder under the selected build's class rules;
mesh components follow explicit blueprint links from the mesh-network component.
Sound graphs are outside the entity-bus traversal. Unknown classes, cycles,
duplicate mesh targets, unresolved references and invalid defaults fail preparation.

Generated version-2 profiles use the same parser as the server. Part definitions
remain in `vehicle-parts.json`. There are at most 512 profiles, 512 serializers and
128 mesh components per profile, with a 16 MiB document limit. No car-specific
profile array, mesh list, tuning value or account identity is embedded in code.
The codec preserves spoiler values, but initial state for spoiler and light
extras remains unsupported. Generating a profile does not establish live support
for every vehicle or customization combination.

## Startup sub-levels

`AssetIndex::startup_entries(level)` lists what a world host names and
registers when the level starts: auto-loaded Win32 sub-levels walked
breadth-first from the `LevelData` (with parents), then bundle preloads and
blueprint bundles from those scenes' layers. Sibling order is the server's
choice; it only has to be used consistently for level IDs.

The generated version-5 world document assigns name IDs from that order and
uses the owned registration allocator for level and stream IDs. Name batches
contain at most eight entries and fit the wire byte bound; registrations are
batched in groups of 64. Parent IDs refer to the generated order. Required scene
roles resolve by name. Traffic slots instead follow level-reference serializer
order in the root composition graph. The customization timer and streaming gate
resolve their class and index from the appropriate scene registry.

The load descriptor selects the authored Game/ConquestLarge policy. Registration
field values are supported host policy, not extracted session messages. Historical
player-car names are unnecessary: the vehicle registry registers selected cars
as they are loaded. Other configuration containing level IDs must use this same
startup order; an explicit older world document remains supported.

Tests build synthetic installations (`nfs-frostbite` `synthetic` feature) and
cover loader acceptance, cache reuse, damaged and foreign entries, changed
installations, unsupported executables, wrong template location or digest,
missing assets and tables, dangling references and invalid field values. They
do not establish that a client accepts content built from a real installation;
that comparison and live validation happen outside this repository.

## Scene catalog

The builder starts at the build profile's level, follows its startup sub-levels,
and visits construction edges in order: Objects, Object, Components, Blueprint.
Internal and imported pointers retain their instance identity. Repeated prefab
occurrences contribute separately; an active-path cycle is an error. Limits are
32 edges of depth, 50,000 visited objects per scene, 512 serializers per scene,
256 profiles, and a 2 MiB catalog. Unknown classes and malformed graph fields
fail preparation; known classes without a server serializer are explicit null
rules in the build profile.

Blueprint bundles have a common no-op scene owner. Their blueprint instances
belong to separate entity replication. Startup bundle references and every
installed bundle targeting a RaceVehicleBlueprint are discovered from assets.
The catalog includes launcher indexes derived from class roles and graph order.
It contains no scene-specific index tables, player identities or captured values.

The server accepts this catalog with version 5 world configuration, allowing
scene_profiles and launchers to be omitted. The generated world document uses
this form. Vehicle, garage logic, sequence and progression inputs and service policy
remain separate. Generation and portable tests do not establish live game or
offline-play acceptance.

## Garage layout

The main garage is the unique constructed GarageEntityData with one primary and
four secondary slot links. Link order determines slot ordinals; serializer
indexes come from construction order. Each VehicleSlotEntityData has one Location
property driven by a constructed LocatorEntityData's Transform. The builder
preserves all twelve stored float bit patterns and adds the four zero padding
words the runtime layout requires. PersistentGarageVehiclesEntityData supplies
the corresponding five ItemEntityData bindings by their class-level ports.

Build profiles select port identifiers; installed connection arrays supply all
scene-specific targets, indexes and transforms. Null/dangling links, duplicate
slots, ambiguous occurrences, unsupported property flags and nonfinite or invalid
transforms fail preparation. No account inventory or runtime identity is read.
The generated layout includes exit_teleport, resolved from the SpawnPoints
interface's exported location property and its constructed locator. It checks the
export direction and connection flags, and rejects missing or ambiguous exports.
The initial world_spawn pose remains separate deployment configuration, pending
complete world-exit content support; an explicit --garage-layout takes precedence.
