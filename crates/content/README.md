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
   the executable, both layouts, both catalogues and every superbundle TOC.
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
| `WorldMacTemplate` | `world-mac-template.bin` | 64 bytes of the executable image at the build's known address, checked by digest |

Derived defaults are the stored 32-bit patterns of each class's same-named
fields (floats as their bits), three words per colour/material vector and
eight NUL-padded bytes of licence-plate text. Timed discounts have no static
default. The outputs use the existing versioned formats, so the server's
loaders and runtime behavior are unchanged.

## Build profiles

Identifiers that belong to one executable build and cannot be read from the
installed data files live in [`builds/`](builds/README.md), one JSON file per
build: the executable digest, the MAC template's address and digest, and the
runtime class IDs of Blueprint classes. `BuildIdentity::supported()` loads the
profile of the build the loaders accept; adding a build means adding its file.

## Asset identities

`assets::AssetIndex` derives what the client uses to name assets: a bundle's
scene content key (djb2-xor of its lower-cased path), its asset catalog
(Blueprint objects in `<bundle>_networkregistry_win32`, counted by class ID)
and asset references (class ID and position in registry order). A Blueprint
class without a profiled ID is an error, never a guess. These are inputs for
world, garage and progression content, which are not generated yet.

## Startup sub-levels

`AssetIndex::startup_entries(level)` lists what a world host names and
registers when the level starts: auto-loaded Win32 sub-levels walked
breadth-first from the `LevelData` (with parents), then bundle preloads and
blueprint bundles from those scenes' layers. Sibling order is the server's
choice; it only has to be used consistently for level IDs. Scene profiles
and launcher serializer lists are not derived yet.

Tests build synthetic installations (`nfs-frostbite` `synthetic` feature) and
cover loader acceptance, cache reuse, damaged and foreign entries, changed
installations, unsupported executables, wrong template location or digest,
missing assets and tables, dangling references and invalid field values. They
do not establish that a client accepts content built from a real installation;
that comparison and live validation happen outside this repository.
