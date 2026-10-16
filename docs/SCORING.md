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

Portable tests use constructed inputs and cover typed register aliasing,
conversion, divide-by-zero retention, interpolation, extrapolation, finite
bounds and malformed assemblies. Installation comparisons are separate from
live acceptance. No captured output, extracted content or private dependency
is required to build or test these modules.
