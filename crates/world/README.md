# Owned world host

`nfs-world` composes the transport core with typed player, participant, scene,
vehicle, inventory binding, garage presence and sequence state. `session::Host`
takes datagrams plus an injected clock and random source and returns outgoing
datagrams and events. It owns no socket, filesystem, global account or recording.

The host queues typed replication sections and ordered RPC notifications. It
checks observer bindings, object lifetimes, loading reports, resource bounds and
backpressure before committing a queued change. There is no captured-frame queue.

Traffic-goal records encode a bounded allocation counter and an explicit asset
reference using the installed spline catalog. Creation, empty updates and
deletion are supported by the replication codec; traffic population and simulation
are tracked separately in [#54](https://github.com/afterhours-crew/afterhours-2015/issues/54).
Wandering goals use the same allocation counter and retain three constructor
flags, with no asset or incremental body. Owned AI assignment and cleanup cover
both goal types; this codec support alone does not establish live rivals.

Supply scene serializer catalogs and current registration data explicitly. Root
traffic slots, gameplay/startup roles and garage scene selection belong to
deployment configuration; the library contains no game content-key table or asset
files. Codecs reject unsupported shapes instead of guessing successful replies.

Entity profiles include RPC initialization with one Boolean or a 16-bit-counted
list of 13-bit object references. Both have empty update bodies. Lists are capped
at 1,024 entries and owned state checks every reference before accepting the
creation. These codecs support driving-query composition; they do not calculate
or award REP by themselves.

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

Player views have typed base and view serializers and one owned instance per
player. Vehicle binding updates the manager, participant and view atomically;
detachment clears view seats before vehicle deletion. Creation and retry handling
share the listener transaction, so full output queues cannot leave unpublished
view state. Numeric view fields whose meaning is unresolved remain explicit.

Traffic components support RPC-only initialization and empty updates. The shared
vehicle constructor also accepts explicit component inputs without an inventory.
Players keeps a separate bounded AI registry in the shared ghost namespace:
participant, actor and car creation commits together, retry keys are per session,
and goals and connection cleanup preserve ownership and roll back failures.
Manager bindings atomically retain Participant/Actor references and return their
add/bind RPCs. Deletion releases those references with the AI objects, matching
client death listeners; failed deletion leaves all retained state intact.
Actor configuration is explicit. These APIs do not yet establish live traffic,
client delegation or rival behavior; those remain tracked in issue #54.

Driver callbacks accept an explicit map of currently delegated AI lifetimes,
separate from owned inventory. Nitrous changes its mode, the horn changes its
chassis flag, and burst/reinflate requests change only the selected wheel.
Drift exit returns an event for a gameplay hook without inventing a reply or
awarding REP. The listener commits callbacks with movement, reliable ordering
and bounded output queues; a failed frame publishes none of those changes.
See [#120](https://github.com/afterhours-crew/afterhours-2015/issues/120) for the
remaining live validation of these paths.
