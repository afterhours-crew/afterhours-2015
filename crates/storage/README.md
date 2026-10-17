# nfs-storage

Synchronous persistence boundary for account-owned inventory, five ordered garage
slots, typed persistent tables and service state. Server callers must run storage operations off
async workers. `MemoryRepository` and `SqliteRepository` implement the same
`InventoryRepository` interface and share transition validation.

- Account identifiers come from owned local configuration. Each SQLite account
  uses its own database; no game account credentials or shipped profile is needed.
- A batch validates the final item graph, garage references and bounded tables
  atomically. Failed batches leave the previous state intact.
- Batch IDs provide retry detection within a bounded history; generations reject
  stale updates. Callers must keep a batch ID associated with the same operation.
- SQLite schema version 5 migrates earlier inventory/garage/table layouts. Ownership
  and version checks precede adoption; successful writes commit before returning.
- Identity, entitlement, kickback, speedwall and settings each have a bounded,
  revisioned table in the same account file. Domain codecs validate their named
  values; the storage adapter knows no protocol replies. Missing domains can be
  imported atomically without overwriting existing values. Compare-and-swap
  rejects stale writes; service revisions are independent of inventory generations.
- Fresh-account publication combines all five service domains with the initial
  inventory/garage/table batch in one transaction. Only a wholly untouched store
  is eligible; retries cannot reset existing state or leave half the domains saved.
- Livery records use a separate table in the same account database. Each complete
  upload commits atomically; an identical retry preserves its bytes and timestamps.
  Names are bounded database keys, never file paths. Limits are 256 records,
  512 KiB per record and 16 MiB of payload per account. Replacements enforce the
  final size, and failed writes preserve prior records. Payloads remain opaque.
  These writes do not advance the inventory generation or alter progression.
  Record metadata uses `records::UnixMicros`; inventory timestamps remain in
  milliseconds. Callers must supply the appropriate clock units explicitly.
- SQLite waits up to two seconds of busy-handler sleep for each contended lock.
  This tolerates brief concurrent writes and filesystem stalls. It is not an
  overall operation deadline; callers retain their bounded blocking workers and
  request deadlines. Persistent contention returns `Error::Busy`, without a
  success acknowledgement or a partially applied transaction. Retry the original
  operation using its existing identity after the contention has cleared.
- Keep account databases on a local filesystem. On Windows, the adapter converts
  canonical verbatim drive paths to normal drive paths before passing them to
  SQLite, and verifies that both resolve to the same directory. This prevents
  SQLite from treating a local drive as a UNC share during concurrent WAL opens.
  Network/device paths and names whose meaning changes under that conversion
  are rejected. Lock waits and transaction durability settings are unchanged.
- The legacy `settings-<account>.sqlite` file is a read-only migration input.
  Its value and revision are preserved; new settings writes use `<account>.sqlite`.
  Stop older servers before migration. Keep the old file as a backup, but do not
  run an older server against the upgraded directory or treat it as current state.
- Definitions and class-specific item bodies are caller-supplied data. This crate
  ships no item catalog, extracted game content or wire protocol implementation.

```sh
cargo test -p nfs-storage --locked
```

The contract suite covers adapter equivalence, rollback, retry handling, account
isolation, resource limits, schema migration and reopening durable state. It uses
synthetic values and temporary storage. Process crashes, power loss and gameplay
persistence are separate acceptance tests and are not claimed here.
