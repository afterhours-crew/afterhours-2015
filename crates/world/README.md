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

Scene profiles distinguish trigger-volume endpoints from ordinary Boolean RPCs.
A trigger volume starts enabled so its client collision listener exists; ordinary
Boolean endpoints retain their inactive default. The world session accepts bounded,
ordered enter/leave reports only for its owned world cars and drops observations
when those cars are removed. Reports do not synthesize cinematic completion.

Common-world activation subscribes each eligible participant to configured volume
filters and notifies the scene of world occupancy and streamed-volume membership.
The model resolves current scene IDs/serials, commits complete batches within the
caller’s queue capacity, and accepts owned, ordered crossing/location reports.
Race-event eligibility is separate; this model does not enable all event volumes
or award progression from a crossing report.
