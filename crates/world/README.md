# Owned world host

`nfs-world` composes the transport core with typed player, participant, scene,
vehicle, inventory binding, garage presence and sequence state. `session::Host`
takes datagrams plus an injected clock and random source and returns outgoing
datagrams and events. It owns no socket, filesystem, global account or recording.

The host queues typed replication sections and ordered RPC notifications. It
checks observer bindings, object lifetimes, loading reports, resource bounds and
backpressure before committing a queued change. There is no captured-frame queue.

Supply scene serializer catalogs and current registration data explicitly. Root
traffic slots, gameplay/startup roles and garage scene selection belong to
deployment configuration; the library contains no game content-key table or asset
files. Codecs reject unsupported shapes instead of guessing successful replies.

Portable tests construct their inputs. They cover connection ownership, retries,
disconnect cleanup, object allocation, transactional updates, resource limits,
replication pacing, files and vehicle initialization. These tests establish the
covered protocol/state behavior, not a complete game or launcher independence.
