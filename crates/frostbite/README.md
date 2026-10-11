# nfs-frostbite

Read-only, bounded access to an installed game's Frostbite 2014.4 content.

- `dbobject`: tagged DbObject containers (`layout.toc`, superbundle `.toc`,
  `.sb` bundle manifests), including the `00 D1 CE 0x` header and its keyed XOR.
- `cas`: `cas.cat` catalogues, block records (stored, zlib, LZ4) and delta
  records that rewrite base blocks (`casPatchType` 2).
- `ebx`: self-describing EBX partitions decoded into named field values;
  numeric values keep their stored representation.
- `install`: an installation with its optional patch layer, an index of every
  EBX entry by name, and record reading. Bundle manifests are read by offset;
  `.sb` and archive files are never loaded whole.

Every reader applies `Limits` (container and asset size, nesting depth,
collection elements, decoded values, bundles and entries) and returns typed
errors instead of panicking. Nothing is written to the installation.

The formats were recovered by inspecting the supported installation. No
third-party reader is included or adapted. The optional `synthetic` feature
provides writers for constructed containers, partitions and installations; the
crate's tests and dependent crates' tests use it, so no game data is needed to
build or test. Decoding a real installation is validated outside this
repository.
