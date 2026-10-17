# Driving-score calculations

Free-roam REP work is tracked by [#50](https://github.com/afterhours-crew/afterhours-2015/issues/50)
and its [input contract subissue](https://github.com/afterhours-crew/afterhours-2015/issues/85).
The scoring participants are not enabled yet. These libraries do not by
themselves award REP or establish live driving support.

`AssetIndex::scoring_calculations` reads math and curve nodes from an installed
blueprint composition. Each node retains its asset, object index and prefab
instance path. Repeated prefab references must have separate runtime state.
Other node classes are outside this reader's scope: successfully compiling the
calculations does not mean the complete graph can run.

`nfs_world::scoring::math::Program` validates a bounded scalar instruction stream
before evaluation. Integer, float and Boolean registers are separate. Reads of
undefined registers, unsupported instructions/functions, nonfinite values and
out-of-range conversions fail. Each evaluation starts with fresh registers;
state such as cached scores belongs to the enclosing per-session graph.
Division by zero returns `Ok(None)`: retain the prior output and publish no new
value. The supported `round` operation adds 0.5 and truncates toward zero,
including for negative inputs. Scalar power uses the platform math library, so
last-bit equivalence across platforms is not promised.

`nfs_world::scoring::curve::Curve` evaluates the default reaction-curve trait.
It scales and clamps the input to the header range, normalizes it, evaluates a
linear or cubic segment, clamps the normalized result, and restores the output
range and scale. Knots need not span the entire normalized domain: the first
and last segments extrapolate to the header boundaries. The content reader
retains the curve's default input and apply flag; the graph must publish zero
when that flag is false. Unsupported traits fail during compilation.

`AssetIndex::scoring_events` reads the server event topology with authored row
order intact. It retains separate prefab instances and explicit forwarding
endpoints across their interfaces. Client-only rows are excluded without
renumbering remote rows. Its composition bus IDs must be bound to the network
replication profile before encoding; not every composition bus is replicated.
Unknown realm values, unresolved endpoints and malformed prefab boundaries fail.
This reader accepts node identities, not their behavior: a complete runtime must
still validate every active node class and its property/link bindings.
The returned composition also includes raw property and provider-link rows with
original bus/row identities and separate prefab endpoints. These tables include
client-side nodes. Property-row order is not listener registration order, and
provider links must not be interpreted as scalar property aliases.
Property connections also retain their complete required 32-bit Flags value;
provider links have no such field. Unknown bits are preserved. Interpreting
connection realms remains separate from each entity's listener realms.

`nfs_world::scoring::events::Graph` dispatches local events synchronously. Nested
events finish before the current handler continues or the next sibling runs;
collected remote notifications follow the local traversal. Dispatch evaluates a
private candidate with bounded depth, work and output. A failure discards that
candidate even if a node handler ignores a nested error. Handlers must keep all
mutable state private to that candidate. The caller commits state and bounded
network output together; handlers must not perform I/O or persist awards.

`scoring::reports::Binding` decodes the near-miss four-float report and the
accumulated-collision integer report from an explicitly bound query serializer.
Every matching message checks participant ownership, current entity/serializer
identity and caller-provided readiness. Report fields are published in order,
followed by the query event. These are measurements, not REP amounts; graph
calculations and the durable award transaction remain separate. The codec checks
shape and finite floats, not physical plausibility. The transport must discard
duplicate reliable messages, and retired participants must lose their bindings.
No scoring report is registered in the live server yet.

`AssetIndex::scoring_nodes` reads float/integer cache, accumulator, float/integer
counter, Boolean value, conditional float, event gate, event splitter and scalar
comparator configurations, retaining authored
defaults and separate prefab occurrences. It does not validate other classes
or property wiring. Connected input properties take precedence over defaults.

`scoring::nodes::Node` owns transient node state and returns ordered property
publications and events. Initialize output properties during binding, then
activate after inputs are bound. Apply each returned effect synchronously after
retaining the changed node state, within the enclosing graph transaction. Cache
loads and accumulator operations publish before notifying, including repeated
equal values. Counter reset restores its authored default. Run-once splitters
mark themselves fired before downstream handlers can reenter them.

Explicit comparisons emit every currently true condition in order without
changing the previous property relation. Property changes emit only conditions
that became true; changes before activation are deferred. Float comparisons emit
inequalities only, while integer comparisons also emit equality or inequality.
Boolean comparisons support repeated notifications through `AlwaysSend`;
explicit evaluation does not change their previous property relation. Integer
counters wrap at signed 32-bit boundaries. Conditional floats publish only the
selected branch when that input changes, and reevaluate on condition changes.
Integer-cache initialization currently requires `LoadOnCreate=true`; unsupported
initialization modes are rejected. `scoring::boolean_fold::Node` evaluates And/Or
after activation, reading only connected inputs in numeric In1..InN order. And
stops at an unready input unless an earlier false already decided it; Or skips
unready inputs and stops at true. Empty And/Or yield true/false. Equal results
still publish. The installed reader preserves InputCount and prefab identities;
the caller must resolve the connected-input mask and output binding.
Numeric PropertyCast support publishes float, integer and Boolean values in that
order; string conversion/formatting is not implemented. Nonfinite values and
out-of-range casts fail before any effects escape. Engine save-state restoration
is outside these fresh-session nodes. The complete property propagation engine,
remaining node/query behaviors and durable award integration are still required.

`scoring::properties::Store` holds resolved Boolean, integer, float, vector and
transform cells.
All connected port aliases read one shared current value. Bound-but-unready
properties are distinct from unbound ports. The caller supplies the validated
cell layout and registered listener order; the store does not infer them from
raw connection rows or choose realms and activation order.

`events::Graph::publish` and `Dispatch::publish` model ordinary synchronous
publication: store first, then notify each registered listener, including equal
values. Nested events/property changes complete before the next listener, which
reads the latest shared value. First-ready metadata belongs to each publication,
even if a nested update changes that value. Properties and events share depth,
work and output bounds. Any error discards the complete candidate, including when
a handler swallows a nested error. Fixed property/listener layouts are bounded;
runtime rebinding and alternate queued property contexts are not implemented.
The caller must commit state and network output together after successful
evaluation. External I/O and durable REP writes do not belong in these callbacks.

`Store::with_registrations` accepts an explicit ordered mixture of node callbacks
and property forwarding. Multiple producers forward into a separate destination
cell: the latest publication wins there without overwriting the other producers.
Forwarding reads the current source after earlier nested callbacks and completes
synchronously before the next registration. It shares the graph's cycle/work
bounds and atomic rollback. Initialization publications and registration order
remain the compiler's responsibility; constructing the store does not forward
initial values or infer wiring from raw rows.

Event composition nodes expose independent event-listener and property-listener
realms from entity Flags. Blueprint bus flags are separate, and absent metadata
is not interpreted as a default realm. These listener fields do not by themselves
decide object construction, event connection delivery, or activation order.

`scoring::stopwatch::Manager` models fresh, stopped timers for one realm/update
pass. Start/Stop change effective membership immediately for repeat suppression,
but the active update list changes at the next update. Additions append and
removals swap with the final entry. An update uses a fixed membership snapshot
and reads each timer's current inputs when its turn arrives. Elapsed time uses
the supplied delta, multiplier and context scale; no wall-clock read or assumed
engine scale is hidden in the library.

A positive trigger threshold fires once when elapsed time is strictly greater
than it. The trigger event completes before the current elapsed value is read
for publication, so a callback can reset it. Reset publishes zero before clearing
the trigger latch and does not stop the timer. Timer operations use the same
candidate, bounds and error latch as property/event dispatch. The installed reader
preserves authored update-pass identifiers and defaults; it does not schedule
passes, resolve context scale, restore engine save-state or enable live scoring.

`scoring::property_gate` forwards six typed channels in Boolean, integer, float,
Vec3, Vec4 and transform order. Fresh binding publishes ready inputs if the gate
starts open. An Open transition publishes only when the installed write-on-open
option is enabled; repeated Open does nothing, and other input events close it.
Changed properties pass through only while open. Unready inputs and unbound
outputs produce no publication. Each input is read after the preceding callback;
a nested Close does not cancel a flush already in progress. Errors discard all
state and publications in the enclosing transaction.

Graph values now include vectors and transforms, with finite checks on every
component. `math::Value` reexports that shared value type for compatibility;
the Math VM and numeric casts still accept only their documented scalar inputs.
The gate reader preserves all authored defaults but does not compile bindings,
restore engine save-state or enable a participant.

`AssetIndex::scoring_arithmetic` reads ordered MathOp Add/Multiply/Divide lists
and VectorMathOp Length from the installed composition. Enum numbers and names
must agree; other installed operators are rejected. It retains prefab identities
but does not resolve input/output bindings or activate nodes.

`scoring::arithmetic::Node` folds In0 followed by In1..InN using float32 steps.
Unbound inputs use zero; bound-but-unready inputs suppress publication. Fresh
Out starts unready, activation evaluates, and later property changes publish
even equal results. This MathOp division has different semantics from the Math
VM above: a divisor whose magnitude is below its small-divisor threshold
multiplies the accumulator by positive one million, including negative tiny
divisors and negative zero. The standalone operation evaluator also supports
subtract/min/max; equal extrema select the right operand's bits. Operations,
connected slots and finite intermediate values are bounded and validated.

`Dispatch::vector_length` evaluates its ready vector input after activation. Unbound or
unready input produces nothing. It publishes Length_Float, completes synchronous
callbacks, then rereads the same input cell for Normal_Vec3 using the original
length. This matters when a length callback changes the input. Short vectors
are copied unchanged. Output bindings remain fixed and start unready; unbound
outputs are skipped. The operation shares the graph's fault latch, depth/work
bounds and atomic rollback. Tests cover nested feedback, cycles, signed zero,
threshold boundaries, missing inputs and nonfinite failures. These calculations
do not establish the full graph startup sequence or live REP support.

`scoring::delay::Manager` keeps an insertion-ordered pending queue, with explicit
delta and per-node time scale. Expiration removes the entry before forwarding
its copied context. Reset removes queued work and clears RunOnce; positive
retrigger replaces queued work. Nonpositive explicit input emits immediately
without cancelling existing entries, whereas AutoStart queues even zero delay.
Repeated input respects RunOnce and the installed duplicate-removal option.

Queue traversal keeps a repairable next-entry cursor, so cancellation and appends
during callbacks follow the native ordering. An append after the former last
entry waits until the next update; otherwise it can be visited in this update.
Pending entries are bounded globally and below the native per-node byte-counter
overflow. Context is a caller-defined fixed-size Copy value, not a wire schema.
Recursive engine updates, save-state restoration and time-scale resolution are
outside the supported contract. The caller routes delayed outputs through the
same bounded dispatcher and commits state/output together; no REP is written here.

Portable tests use constructed inputs and cover typed register aliasing,
conversion, divide-by-zero retention, interpolation, extrapolation, finite
bounds and malformed assemblies. Installation comparisons are separate from
live acceptance. No captured output, extracted content or private dependency
is required to build or test these modules.

`AssetIndex::scoring_interfaces` reads each prefab instance's field declarations,
including interfaces outside the object list. It preserves field order and signed
ID bits, distinguishes Source and Target, and accepts empty, Boolean and finite
Float32 defaults. Empty text leaves the field unready. Unknown types, reference
defaults, malformed enum/name pairs, duplicate IDs and oversized input fail.
This is a strict scalar subset, not the engine's general text-value parser.

`Dispatch::register` appends a property listener or forwarder without invoking it
or copying an existing value. This permits construction-time registration in the
verified order. A current publication keeps its listener snapshot; a later nested
publication sees registrations already appended. `Dispatch::initialize` applies
an interface default either only while unready or as an explicit override, as
selected by the binding compiler. Applied defaults notify with `first = true`,
including overrides of a ready value; ordinary publications still derive that
flag from prior readiness. Both operations share dispatch bounds, error latching
and rollback. These APIs do not infer the installed binding modes, startup order
or activation sequence, and do not enable a scoring participant.

`Composition::property_plan` now compiles the installed local connection subset
for one execution realm. It keeps authored input order, first-row mode metadata,
distinct two-producer merges, inherited prefab-output realms and implicit
blueprint outputs. Unused boundary outputs are omitted. Connection flags remain
separate from entity listener realms. `property_layout` resolves the resulting
storage identities: child inputs reuse the parent's resolved input, child outputs
reuse the parent's output, and merge values remain separate from their producers.
Input and output port namespaces are distinct. Repeated producers are preserved.

The reader rejects unverified networked modes, more than two producers, direct
interface bridges and conflicting prefab output aliases. The installed scoring
graphs fit this subset. The result is untyped, per-session topology, not an
initialized executable graph: value kinds, defaults, construction/listener order,
provider links and activation still need integration. Cell numbers are stable
model identities and do not claim the engine's pointer ordering. No live REP
support follows from a successful layout read.

`AssetIndex::scoring_creation_realms` reads creation eligibility independently of
event/property listener Flags and connection delivery modes. It supports the
known scoring entity classes, including fixed client/server/both rules which can
override an authored `Realm` field. Authored enum values require matching names;
unknown classes and the unverified pipeline realm fail. Known blueprints and
interface descriptors are not entities. Every prefab occurrence keeps its own
identity, with bounded traversal and cycle rejection.

This is a creation filter, not proof that a realm has a usable factory or a
schedule for construction completion and activation. Reading the composition in
order alone is insufficient to establish those phases. The scoring participants
remain disabled until the complete runtime contract is integrated and tested.

`AssetIndex::scoring_synchronized` reads Boolean, float and integer synchronized
node occurrences without inventing authored defaults. `scoring::synced::Node`
keeps replication dirtiness; its value lives in a separate typed Out cell that
must exist even when no other node consumes it. Fresh binding copies a ready
input, otherwise leaves Out unready. Activation makes no publication.

`Dispatch::synchronized` suppresses equal ready values, including opposite signed
float zeros. The first ready false/zero still publishes. Publication and nested
callbacks finish before marking dirty and emitting a Boolean OnTrue/OnFalse.
Input-based actions reread the source cell after callbacks to select that event;
explicit SetTrue/SetFalse/Toggle commands retain their own value. An unready
Toggle sets true. Numeric nodes ignore commands. Faults roll back the candidate,
including dirtiness and nested events. Floats must be finite.

Dirty state is pending candidate metadata, not an encoded update or delivery
acknowledgement. A future runtime must commit its state and replication queue
together. Engine save-state restoration, synchronized wire encoding and the
complete scoring graph remain outside this operation; no participant is enabled.

`AssetIndex::scoring_awards` follows each server score node to its context, proves
that its participant provider addresses this participant root, and reads ordered
keeper-channel references. It validates channel declaration types and server
realms, rejects missing or ambiguous singular links, and preserves separate prefab
identities and duplicate keeper occurrences. It supports this own-participant
channel subset; scene keeper resolution and activation remain the runtime's job.

`Dispatch::score` handles Award by reading the current integer Score once, then
the context's participant once, and invoking its activated keepers in order.
An unbound or unready Score input uses the installed authored default. That
fallback must not be mistaken for a completed driving calculation: initialization
must be settled before enabling the graph. Repeated awards and zero or negative
amounts reach the keeper handler unchanged. Other events do nothing; there is no
invented output notification or implicit sum of category awards into total REP.
Keeper callbacks finish synchronously, share graph bounds, and may only modify
the private candidate. Late failures and swallowed nested errors discard it.

This operation does not implement scene keepers, provider rebinding during an
award, signed overflow behavior or persistence. The durable earned-REP policy
separately rejects negative credit and overflow. A runtime still needs to resolve
the owned keeper/channel targets and aggregate a valid credit before committing.

Collision queries now have a separate report binding for their RPC-plus-Boolean
serializer. Client method 0 carries one signed integer and one additional nullable
object reference. Receiving checks the current query owner, serializer identity,
readiness, exact argument/reference shape, and the existence and readiness of any
non-null context. That context may identify something hit by another player;
it never changes who owns the query. Class-specific context resolution and
physical plausibility remain the session interpreter's responsibility.

`Dispatch::collision` retains a query flag independently of its optional outputs.
Begin/Terminate change the flag and enqueue commands only on a transition, then
publish querying. Server command methods are 1 and 2, without arguments. A report
clears the flag, publishes querying and the integer result in order, and emits a
nullable entity-context event. It sends no Terminate command. Nested callbacks
finish before proceeding; a callback may restart the query. Repeated reports
remain measurements, while equal querying values follow the installed binding's
explicit suppression flag. Integer reports always publish when bound.

`Dispatch::emit_context` preserves context through explicitly opted-in handlers
and remote output records. Legacy handlers reject contextual events, including
null contexts. Newly created ordinary events do not inherit surrounding context;
forwarding handlers must explicitly retain it. Shared limits and transaction
rollback cover commands, publications, contextual events and swallowed errors.
The caller commits the candidate and its bounded command queue together.

These APIs do not enable a participant, encode control/event packets, perform
query activation, infer output bindings, resolve native collision filters, or
award REP. The road-driving query needs server-side road/path and vehicle state;
its RPC serializer alone does not establish an incoming measurement method.

`AssetIndex::scoring_roads` reads each road query's eight typed, finite,
nonnegative thresholds. `road::Node::from_state` takes explicit state: fresh
native allocation and save-state initialization are not yet established, so no
startup defaults are supplied by this API. The caller supplies owned road
projection/direction measurements, vehicle speed in metres per second, a resolved
optional speed-limit override, elapsed time and a monotonic double-precision clock.

`Dispatch::road` converts speed to mph, publishes a changed limit first, handles
speeding/debounce next, and wrong-way distance last. Excessive speeding takes
precedence over ordinary speeding and retains its signed integer event payload.
Both speeding duration and debounce comparisons are strict. Wrong-way entry and
recovery have separate strict distance thresholds; dropping below minimum speed
ends the state. Only positive speed-limit overrides apply. Missing road data
preserves query state, while distance at or beyond the road boundary clears
durations and wrong-way state without emitting an invented not-speeding event.

Callbacks complete between phases; all state, properties, integer/contextual
events and remote records share transaction bounds and rollback. Integer-context
events require explicit handler support, just like entity contexts. Native
out-of-range conversion/NaN behavior is not emulated: invalid measurements,
negative durations, nonmonotonic clocks and overflow fail the candidate.

The installed road database, projection and direction classifier, update manager,
startup state, participant binding, RPC/event encoding and durable awards remain
separate work. Constructed transition tests do not establish live road scoring.

`AssetIndex::scoring_property_declarations` derives known input/output types from
the modeled node contracts. Math inputs and returns come from its validated
instructions, including integer inputs and Boolean returns. Unsupported classes
and unmodeled ports stay undeclared. Each prefab occurrence has its own identity.

`Composition::typed_property_layout` matches those declarations to retained
connections and incorporates typed interface defaults without publishing them.
`Layout::property_types` propagates type equality across aliases and merge edges.
Producer and merge values remain separate. Conflicting declarations, foreign
identities, malformed indices and excessive collections fail. Unresolved cells
remain `None`; an inferred type does not establish the producer's behavior.

`Layout::unready_definitions` requires every connected cell to have a type and
preserves the resolved storage identities. It rejects input/output namespace
collisions that would alias distinct cells in a Store. The returned cells have
no values or listeners. The caller still needs ordered registrations, forwarding,
defaults, unconsumed private outputs and activation. Remaining query, telemetry
and UI bindings prevent constructing the complete installed graphs; participants
remain disabled. No type inference result establishes a calculated REP award.

`property_cast::Bindings` accepts the complete numeric input/output binding set;
string, unknown and duplicate ports fail. Binding itself seeds no values.
`Dispatch::property_cast` activates from the first ready bound input in Boolean,
integer, then float order. If none is ready, it publishes nothing. A numeric input
notification works before activation and snapshots that input once. Bound float,
integer and Boolean outputs publish in that order; nested callbacks finish between
them without replacing the outer conversion's input. Equal publications repeat.
Range, type, callback and recursion failures discard the entire candidate.
The caller must exclude neither unknown ports nor string bindings to make this
subset appear supported; string conversion requires its own implementation.

Boolean caches now preserve their installed `LoadOnCreate` flag and Boolean input
fallback. When loading on creation, their initial output uses the resolved input;
otherwise binding supplies no initial value and preserves any shared cell state.
The interpreter resolves an unbound or unready input to its authored fallback.
Input changes and activation alone do not load. Each Load publishes OutValue,
including equal values, before OnLoaded; the event still applies when the output
is unbound. The interpreter skips only the absent output publication, then runs
the event. Engine save-state restoration and the full graph remain unimplemented.

`vehicle_query` models scheduled physics and drift measurements from the query
owner's vehicle. Complete output bindings include explicit equality-suppression
flags. Physics construction offers an identity transform, zero velocity and
false Boolean states; the two RPM outputs have no initial value. Drift offers
zero velocity and angle. These are constructor inputs, not a policy for replacing
shared cells; the interpreter still owns binding and initialization order.

`Dispatch::vehicle_query` resolves a stable owned vehicle handle once. Missing
participant/vehicle/physics data preserves all outputs. Physics publishes velocity,
transform, three Boolean states, maximum RPM and RPM in that order, reading each
measurement after earlier callbacks finish. Two Boolean states remain identified
by their port IDs until their gameplay meanings and owned sources are established.
Drift snapshots transform and velocity before publishing velocity, then publishes
its signed angle in radians. The formula uses atan(lateral/abs(forward)); below
f32 epsilon forward speed it uses atan(lateral). It is not an atan2 heading angle.

Ready equal outputs suppress notifications only when the installed binding asks
for it; signed zeros compare equal. Unready zero/false values still publish.
Optional outputs, nested callbacks and failures use the graph's existing bounds
and rollback. Nonfinite values and arithmetic overflow are rejected. Platform
atan implementations can differ in their final bits from the client CRT.

These APIs do not supply vehicle physics, ownership resolution, participant
activation, update cadence, replication or REP awards. They must be called only
for an active, scheduled query. Installed-data and constructed tests do not prove
live driving support; scoring participants remain disabled.

`draft::Configuration::evaluate` calculates drafting from an owned participant
vehicle snapshot. It excludes the owner's physics identity, skips cars outside
50 metres and applies strict direction, projected-speed, speed-difference and
time thresholds. Time falloff and the projected horizontal cone determine draft
strength. Strength and positive delta speed take independent maxima across cars;
they can come from different participants. Empty sets produce zero/false, while
missing own vehicle data preserves the previous outputs.

`AssetIndex::scoring_drafts` reads the five authored scalar fallbacks. The caller
resolves connected ready inputs before applying these fallbacks and validates the
resolved configuration. Draft publication uses the same bounded query dispatcher:
strength, delta speed, then active Boolean, retaining the calculated snapshot
across callbacks. Query construction supplies zeros/false. The participant-set
resolver, activation/cadence and live driving acceptance remain outside this API.
Malformed thresholds, nonfinite calculations and excessive participant sets fail.

The road-query declaration now recognizes the same class as the strict road
configuration reader, so its verified property ports participate in type checking.

`AssetIndex::scoring_program` compiles one composition's server realm into a
`scoring::program::Program`: per-node behaviors from the strict class readers,
typed unready cells from the property layout, the authored event edges and the
startup sequence in the observed native order (bus construction with merge
forwarders and interface defaults, then each created entity's bind and its
listener registrations sorted by signed field ID, then insertion-order
activation). Every server-created class must be modeled, inert or in the
explicit unsupported list; an unknown class fails. Listener registration honours
the entity's property realm and the connection mode bit; prefab references
register nothing. A synchronized node owns a private Out cell even without a
consumer. The 28 scoring cells whose kind is not modeled touch only telemetry
and channel sinks; the compiler reports them as `unmodeled` and rejects an
untyped cell that any modeled node reads or writes. Unsupported nodes keep their
outputs unready, so consumers fall back to authored values; they are not awards.

`scoring::interpreter::Session` executes a Program for one participant. It is
sans-IO: `start` runs the compiled startup once; `tick` updates every active
query at interval one, then the stopwatch pass, then the delay pass, with the
caller's delta, clock, owned vehicle state, other vehicles, road sample and
time scale; `report` delivers a decoded, ownership-checked query report; `event`
emits a lifecycle event such as the root's disable notification. Each call runs
on a private candidate and commits only on success, returning remote emissions,
collision commands, keeper award requests and dirty synchronized values for the
caller to commit together with its own state. Reads use the native getter
contract: a ready bound cell, otherwise the authored fallback. Math evaluates at
activation and on input changes unless `EvaluateOnEvent` is set, and on any
received event; an input that is not ready suppresses evaluation as a modeling
decision. Curves evaluate at bind and on input changes. Physics measurements
the server cannot supply preserve that output. Road counters start at zero and
missing road data preserves road state. An award carries the installed keeper
link ID; mapping it to a category and persisting credit belong to the server.
Running the installed programs against constructed vehicle profiles is a model
check, not live acceptance; neither participant is enabled by this change.
