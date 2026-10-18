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
| `ProgressionContent` | `progression-content.json` | Main/intro persistence links, stable IDs, activation expressions, entity composition and explicit spawner-to-scene channel bindings |
| `SequenceContent` | `sequence-content.json` | Waiting/exit sequence registries, manager bus and serializer, spawner channel links and the exit command's registered spline definition |
| `TrafficContent` | `traffic-content.json` | Linked traffic personas in provider order, vehicle profiles, manager serializer ordinals and roads with owning scene catalogs |
| `RivalContent` | `rival-content.json` | Roaming racer activation regions, spawn transforms, complete vehicle presets, provider and personality-library indices |
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

Item content version 9 includes explicit REP wrap/decal reward definitions and
their score thresholds, resolved from the progression graph's award transactions.
They are distinct from purchasable part grants. Both use the installed constructor
defaults; no level or reward GUID is hardcoded. Builder revision 25 invalidates
older caches so existing installations receive these rules. Daily-challenge and
story rewards are separate policies.

Item content version 10 adds explicit purchasable car definitions and their REP
requirements. Prices, quantities, stock parts and additional account wraps come
from installed definitions. Nonpurchasable story, traffic and reward cars are
excluded. Builder revision 26 also resolves the garage command serializer from
the installed five-slot link graph. Older content can still display its garage;
ownership commands require the new policy and serializer binding.

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
world, garage, vehicle, sequence and progression content.

Sequence extraction checks the waiting and garage-exit serializer layouts and
resolves channels from each scene's particular sequence referer, through prefab
and scene interfaces. The manager's scene determines the one-based embedded bus.
The spline command selects a channel port; that target selects the exact registered
`ScriptedAISplineEntityData` object and its loop default. Speed-segment assets and
unrelated channels are not interchangeable with these identities. Missing,
ambiguous or unsupported graphs fail preparation. Version 2 contains no goal
allocation counter; that value belongs to owned runtime state.

## Participant logic layouts

`AssetIndex::participant_layout(scene, blueprint, owner)` derives a registered
participant blueprint's asset reference, catalog, bus count, serializer profile
and contextual channel bindings. Scene-owned logic uses the blueprint's unique
installed spawner. Participant-owned children bind only to compatible channels
inside the explicitly supplied parent. Repeated prefab instances retain separate
contexts; missing, ambiguous and unsupported composition fails.

The supported build's rules cover the driving-data and driving-style scoring
compositions, including the distinct RPC-only, single-Boolean and reference-list
query serializers. Server-only queries and local evaluators remain explicit
non-serializing classes. Unknown classes still fail extraction. This enables
layout inspection; driving awards and live scoring acceptance remain tracked in
[#50](https://github.com/afterhours-crew/afterhours-2015/issues/50).

This extraction API supplies static layout only. It does not choose which logic
to activate, initialize account-dependent values or publish another prepared file.
The server's remaining garage lifecycle policy still needs those integrations.

`AssetIndex::participant_boolean_values(blueprint)` derives each Boolean
serializer's initial expression. It supports stored defaults, Boolean operators
and the supported conditional math form. Dynamic leaves identify an exact channel
reference occurrence and a port for the owning participant's property, set or
context state. `Program::evaluate` requires those inputs from the caller and
rejects missing state. Context reporting remains an explicit caller input.
Disconnected fields remain uninitialized; unsupported connected sources fail.
Compilation bounds graph depth and expression size, rejects cycles and checks
the participant provider. BoolEntity defaults describe creation only: this API
does not implement later achievement events or choose garage lifecycle policy.

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
this form. Garage logic, sequence inputs and service policy
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

## Progression

The generated version-1 document uses the same static parser as the server.
Persistence links determine ordered objectives and integer values; StableUniqueId
links supply their IDs. Activation properties support booleans, objective
completion, negation, conjunction and the consumer-beta setting. Participant
property aliases resolve by channel asset and port to a unique publisher across
the two graphs. Missing, ambiguous or unsupported inputs fail preparation.

Entity roles follow construction order and executable class rules. Registered
blueprint flags supply bus counts; the progression scene's registry supplies
asset references. Bundle IDs follow generated startup order. Channel references
bind only to their spawner's ordered target links, following prefab and subworld
interfaces. Repeated prefab instances keep their own context; WorldPart layers
share the enclosing scene's connection tables. Scene serializer indexes must
agree with the scene catalog constructor. No scene-specific binding list is
embedded in the builder.

The builder bounds composition and connection traversal to depth 32 and 50,000
steps per traversal, 500,000 cached connections, expression depth 16 and 8,192
expression nodes. The existing parser bounds objectives/values and the output
to 512 KiB. Default operator settings disable consumer beta and SpeedLists;
an explicit `--progression-content` keeps precedence. Account values and runtime
identities remain owned by the server. Floats in progression persistence and
unrecognized activation expressions are unsupported.

Vehicle mesh inputs retain front/rear headlight animation extent and spoiler
extent/animation ID from the installed item definitions. The vehicle model applies
them to selected mesh variants; non-finite fields and wrong definition classes
are rejected. Older definitions lacking animation inputs remain readable but
cannot construct an animated component's selected part.

## Participant value inspection

`AssetIndex::participant_integer_values` derives initial integer expressions from
participant graphs, alongside the layout and Boolean APIs. Accessor inputs identify
the exact channel occurrence and current participant. Score calculators follow
their installed threshold asset and share the server's signed-score, boundary and
cap behavior. Missing state, ambiguous links, wrong types, cycles and excessive
depth fail. IntEntity defaults apply only to creation; event-driven gates and
later counter mutations remain unsupported. This API does not yet produce the
complete garage lifecycle configuration or add another prepared file.

`AssetIndex::network_events` enumerates client-networked event bindings from a
scene or participant blueprint. Registered bus occurrences follow composition
order; event ordinals retain their position in the complete connection table,
including local-only rows. Endpoints retain their class, port, referenced prefab
and any named presentation context. Endpoints must belong to that exact bus
occurrence or its own interface. Unknown delivery types, ambiguous occurrences,
unregistered network buses and excessive table sizes fail extraction. The caller
still owns event timing, lifecycle policy and live identities; this API fires no
events and adds no prepared runtime file.

`vehicle_profiles::traffic::extract` resolves a requested installed traffic
bundle's component order, asset catalog, property buses, physics and health
inputs. Its strict `Definition` omits player customization and inventory fields;
player definitions retain their existing requirements. Unknown components fail
extraction. The prepared traffic document also follows manager road links through
subworld interfaces and keeps each road's scene, local catalog index and catalog
count separate from population order. Persona identities follow their provider's
member links. Unknown providers, missing scene registrations and ambiguous links
fail preparation. Version 3 rebuilds older traffic caches. Completely collapsed
authored curve segments remain in the road and speed tables but have no spawn
tangent. Population and AI lifecycle belong to the server.

`rivals::extract` resolves the roaming-racer manager, its participant/actor
bindings, authored activation volumes, spawn transforms and linked personas.
Specific providers require all configured tag hashes. Persona styles construct
ephemeral item collections with ordered part/kit replacement, paint, alignment
and rim settings; explicit persona additions follow the style. They allocate no
account items or garage slots. Performance additions are applied before chassis
quantization and grip overrides preserve the transmission selection.

The `rivals` example audits these inputs against an installation and an existing
prepared-content directory. The server's `rival_vehicles` example additionally
constructs and encodes complete vehicle components with synthetic mesh identities.
Preparation writes `rival-content.json` with those inputs and the native ordered
personality-library indices. The server consumes this bounded cache for its
session-owned rival population; the audit examples alone do not prove live driving.
