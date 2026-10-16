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
initialization modes are rejected. The separate bounded Boolean fold helper
distinguishes unconnected ports from bound properties that are not ready; it is
not yet a complete And/Or binding implementation.
Numeric PropertyCast support publishes float, integer and Boolean values in that
order; string conversion/formatting is not implemented. Nonfinite values and
out-of-range casts fail before any effects escape. Engine save-state restoration
is outside these fresh-session nodes. The complete property propagation engine,
remaining node/query behaviors and durable award integration are still required.

`scoring::properties::Store` holds resolved Boolean, integer and float cells.
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
restore engine save-state or enable a participant. VectorMathOp interpretation
remains separate and unsupported.

Portable tests use constructed inputs and cover typed register aliasing,
conversion, divide-by-zero retention, interpolation, extrapolation, finite
bounds and malformed assemblies. Installation comparisons are separate from
live acceptance. No captured output, extracted content or private dependency
is required to build or test these modules.
