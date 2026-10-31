# Story implementation workflow

This is the implementation and review contract for story/phone work. Gameplay
scope and progress remain in the [story tracker][story] and its sub-issues.
It does not establish support for any mission or authorize a release.

## Agree on a complete batch

Before gameplay implementation, present one coherent batch to the owner. Include:

- Starting account/objective state, named missions and an exact endpoint.
- Calls/messages, events, cutscenes, rewards and unlocks included in that span.
- The following handoff and saved restart; specify what remains outside scope.
- Existing mechanisms to reuse, missing mechanisms and dependencies on other
  systems (for example event families, police or multiplayer).
- Evidence coverage, unresolved questions, automated acceptance and the owner's
  live-run observations. Give a workload/risk assessment without inventing a date.

Record the owner's approval and scope in the task. Inspection and documentation
already requested by the owner may proceed before this approval. Once approved,
work through related fixes, checks, commits and PR preparation without asking
again at each step. Seek a decision only for material scope expansion, destructive
owner-state changes or an action reserved to the owner. Continue independent work
while a decision is pending. Merge and release authorization remain separate.

## Establish the transition contract

For every objective, establish prerequisites, start trigger, participants, event
variant, success/failure conditions, interruption/retry behaviour, presentation
handoff, durable effects and next objectives. Multiple objectives may be active;
do not assume a single linear chapter or that winning always completes a mission.
Distinguish ordinary event rewards from story completion and repeatable rewards
from once-only unlocks. Specify when each effect becomes durable and observable.

Use authored conditions and observed behaviour together. A recording establishes
an observed path; it does not prove the cause of a transition or every permitted
order. Tester descriptions are leads until corroborated. Unexpected behaviour in
an official session is not automatically a requirement to reproduce a bug.
Missing evidence remains explicit and blocks the affected readiness claim.

Ground RPC reference kinds and ownership as well as their argument bits. A valid
ghost ID and successful encoding do not prove that a method expects that object:
a player and its joined participant are distinct references. Check the expected
kind against the client reader and observed call, and retain a control that
substitutes a different live object kind or another owner's object.

Keep detailed reference material in the private research workspace. Public tasks,
code and tests contain reviewed semantics and constructed inputs, never captured
identities, raw traffic, profile data, extracted content or private source links.
No production build or runtime may depend on the reference workspace.

## Reuse mechanisms and preserve ownership

Extend the existing content, sequence, event and progression components. Resolve
authored mission data through validated content; do not encode captured timelines
or clone a mission-specific handler for every new objective. Generalize only the
shared mechanism required by the agreed batch, with the existing prologue kept
as a regression. Inspect intro-only assumptions before using them for main story.

Objective state, rewards and durable operation receipts must commit together
where the transition requires it. Exact retries must not duplicate once-only
effects. Publish mutually dependent client updates in the required order/batch
after successful persistence. Test commit failure, stale generation, restart and
account isolation. Preserve existing saves through explicit versioned migrations.

Use bounded, correlated diagnostics for stalled objectives: last accepted input,
unmet condition, awaited response, pending commit, relevant entity lifetime and
deadline. Unexpected timeout recovery or unsupported work remains a failed or
degraded case. Containment prevents unrelated movement from failing; it does not
turn the affected mission into a pass.

## Validate the flow and its boundaries

1. Retain a failing case, the corrected case and a related case exercising the
   same mechanism. Where appropriate, retain a known-correct control too.
2. Exercise evidence-supported order/delay variations, duplicate reports,
   interruption and retry. Label constructed fault controls separately from
   observed client behaviour. Do not invent successful client responses.
3. Assert exact objective flags, reward amounts/items, unlock conditions,
   ownership, cleanup and the next objective. Message counts, successful
   serialization and transaction counts are insufficient.
4. Compare generated output across the whole agreed span, including presentation
   at transition boundaries. Test the following ordinary-play/garage handoff and
   a server restart on a copy of the resulting save. Separately observe the real
   client restart. A later recovery does not repair a wrong boundary state.
5. Run the contribution gates and private reference comparisons. When a public
   library revision changes, qualify its consumers early on that exact pin; do
   not defer combined compilation until the final merge. Identify every source,
   executable, starting save and content revision used in validation.
6. If packaging/launcher behaviour changes, test the actual paired artifacts,
   installation/first-run startup, authoritative profile refresh and ordinary
   shutdown. Preserve the measured warm-start budget and prove cache reuse after
   content changes. Cache immutable definitions, never live mission state.

The reference driver must report which observed input classes/transition paths it
does and does not exercise. Every new live failure feeds a maintained regression.
Missing or unhealthy raw evidence cannot be replaced by a green summary. Tool
qualification remains separate from gameplay acceptance; rendering, exposure,
controls and responsiveness still require the owner's observation.

## Review and handoff

Report implemented, offline-tested and game-validated separately. Provide the
exact source/artifact identities, first failing phase if any, changed assertions,
related control, remaining uncertainty and a short owner run plan. Keep one
current task status; append historical evidence without rewriting failed runs.

Within an approved batch, agents can commit/push complete checkpoints and prepare
the PR under existing repository rules. They do not create another automatic
review loop or treat a source merge as release approval. If scope remains
unresolved, report the concrete decision rather than claiming readiness.

[story]: https://github.com/afterhours-crew/afterhours-2015/issues/56
