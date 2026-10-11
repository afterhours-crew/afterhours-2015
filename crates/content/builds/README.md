# Build profiles

Each file describes **one executable build** of the game. Values here are not
game content: they are build-specific identifiers the content builder needs
and cannot read from the installed data files. They change whenever the
executable changes, so every supported build gets its own file.

File name: `<executable stem, lower case>-<first 16 hex digits of its SHA-256>.json`,
for example `nfs16-92aa6ff4b5f8d0f0.json`.

| Field | Meaning |
| --- | --- |
| `format`, `version` | `nfs-build-profile`, `5` |
| `description` | Human-readable identification of the build |
| `executable`, `executable_sha256` | File name in the installation root and its exact digest |
| `world_mac_template.rva`, `.sha256` | Relative virtual address (hex) of the 64-byte world MAC template inside the executable image, and the digest the bytes must have. The bytes themselves are read from the player's executable, never stored here |
| `blueprint_class_ids` | Runtime class IDs the client assigns to Blueprint classes. Asset catalogs and asset references sent to the client use them; they are not stored in the content containers |
| `garage_links` | Port IDs for the primary/secondary garage slots, location/transform property, five persistent inventory-slot roles and the SpawnPoints exit-location export. These select installed connections, not fixed scene indexes or transforms |
| `vehicles` | Vehicle class-to-serializer rules, bus and mesh-extra classes, mesh-link port, and player-bundle namespace. Profiles, mesh lists and defaults are read from installed assets |

Only the classes whose IDs are known are listed; building content that needs
another class fails with an explicit error rather than guessing.

To add a build: create its file with every field, add it to `PROFILES` in
`src/builds.rs`, and keep the tests passing (they validate every profile).
Content documents for that build also need the loaders to accept its hash.

The scenes object selects the level asset and lists executable-specific class
construction rules. classes maps a class name to a stable serializer kind or
null (a known class with no server scene serializer). Missing classes fail
preparation. launchers lists classes whose emitted serializer is a launcher
endpoint; each must have an RPC-compatible kind. These are type-level protocol
rules, not scene instances, per-scene serializer arrays or game asset content.
The cache fingerprint includes all profile fields.
