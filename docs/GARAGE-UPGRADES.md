# Garage upgrades and customization

The server models supported leaf performance and visual-part purchases,
plus explicit body kits containing supported visual leaves,
their equip/save requests, and restoring stock parts. The install-content builder
selects supported offers from the installed definitions. Prices, part GUIDs and
owned item IDs are never copied from a saved account or hardcoded in the server.
The car's own installed compatibility list restricts both purchase and equip;
an offer for a different car cannot be bought or fitted just because it exists
in the catalog. Recorded stock defaults remain available to restore.
Installed fitment restrictions also apply to supported leaf parts. Equip is rejected
when the selected part conflicts with equipment that would remain on the car.
Restricted parts can still be purchased as spares. Parts in storage and the old
part in the replaced slot do not block the selection. Front and rear slots are checked separately. Removing or
replacing a conflict earlier in the same edit allows a later compatible selection.
The server also stages the 31 native handling categories against installed parts and
the car's transmission flag. REP-only offers use the account's saved score and
installed thresholds. Visual parts match exact class and, where applicable,
front/rear location. Nested kits, kits containing library liveries, separate
story/build restrictions and selling remain unsupported. Saved wraps can be restored from the local library and
equipped. The live checks below cover selected performance
and visual parts, tuning, paint, wraps and stock restoration. The acceptance and
explicit boundaries of this milestone are tracked in the
[garage upgrades issue][garage-issue].

Body-kit purchase and equipment support is validated for the tested Mustang Race Kit in
[#73](https://github.com/Afterhours-Crew/afterhours-2015/issues/73). Content version 8
explicitly lists eligible kit offers; earlier content formats never enable them.
The builder regenerates its cache for this version. A purchase allocates the kit
and its children in one transaction, charging only the kit price. Children belong
to the kit, which belongs to the car. Retrying returns the existing owned tree.
Equipping replaces overlapping slots and retains the previous kit and individual
parts as spares. Replacing a kit restores uncovered mandatory stock visual slots;
a later individual car-owned part can override a kit slot. Stock-visual reset
removes the installed kit while retaining its ownership. Direct selection of a
kit-owned child as a car-owned part is not supported. The tested kit locks covered
individual-part menus; the game offers stock restoration before changing them.
Portable tests cover the
transaction, cancellation, account isolation, concurrent retries, stale edits,
replacement, stock restoration and database reopen. These tests do not establish
working behavior for every body kit in the game.

The 2026-10-15 live check canceled the kit purchase without changing the account,
then bought and equipped it for the installed price. The kit appeared in free
roam, survived garage return and a full game/server restart, and retained the
existing wheels and plate text. Restoring stock except wheels removed the installed
kit while preserving all owned items. A previously owned hood could then be
equipped; the saved kit could also be re-equipped without another purchase or
charge. The final balance was unchanged after those equipment operations.
The supported purchase reply explicitly indexes the kit and every child record.
Omitting those child index entries caused a client crash after purchase in an
earlier candidate; the corrected purchase completed successfully. The captured
request and reply round-trip through the typed codecs, and independent reference
comparisons check owned kit graphs, defaults and prices. All 1,257 portable tests,
formatting and Clippy pass. This coverage does not claim nested kits, library
livery bundles, every kit/part combination, or a way around the game's restrictions.

The combined live test on 2026-10-14 equipped an owned air filter and purchased
a compatible cooling upgrade in one performance edit. Both parts, the exact
cash deduction and client-calculated dyno figures survived a garage/world/garage
loop and a full game-and-server restart. Front tire pressure, rear tire pressure
and steering response also survived both transitions. Earlier runs exercised
stock restoration and driving with the filter. The split-message receiver fix
resolved the previously lost dyno cache.

On 2026-10-15 the owner reported a noticeable difference while driving compared
with the car before performance parts were installed, and accepted that the parts
affect actual driving. This is a qualitative owner-validated result for the tested
configuration; it does not measure the displayed performance figures or validate
every handling control. Stock-performance reset subsequently passed a full restart:
the garage retained the stock parts and displayed 257 hp, 5.66 s 0–60 and 144 mph.
The purchased upgrades and nitrous remained owned spares, with appearance and cash
preserved. These displayed figures are saved client dyno values, not road measurements.
The owner then restored stock performance and selected full-grip tuning, reporting
less sliding while driving. This validates the observed combined configuration;
the test changed parts and tuning together, so it does not isolate either change
or every slider's contribution.
The owner then installed eight performance upgrades and reported that the car was
definitely faster. The transaction audit accounted for all eight upgrades and the
two visual purchases, including the exact remaining balance. It found no nitrous
purchase in that batch. A separate explicit test bought and equipped the 5 lb
time-refill nitrous kit for its catalog price, retained it through a garage return
and normal shutdown, and showed blue exhaust jets when activated while moving.
The installed label and balance survived a full game/server restart. A subsequent
boost visibly depleted the gauge, which returned to full after release. This
completes [the nitrous test](https://github.com/Afterhours-Crew/afterhours-2015/issues/72)
for this kit; exact power gain, refill duration and other variants were not measured.

Both stock-visual reset choices passed a subsequent combined test. “Except
Wheels” restored the stock body parts while retaining the selected wheels and
their finish through free roam and garage return. “Including Wheels” selected
the stock rim, tire and caliper definitions. The final stock parts survived free
roam, garage return and a full game/server restart. Correcting the reset option's
one-bit decoding resolved the rejected request that previously stalled later
garage actions. Portable regression tests and actual-request comparisons cover
both options and their bounds.

The paint library's Remove action restored factory body paint and removed the
equipped body decals. The gray body and cleared windshield survived free roam,
garage return and restart; installed performance parts, nitrous and plate text
were retained. The stock rear wheel still had its previously saved green finish.
These checks establish the tested part and body-paint resets, without claiming
that resetting parts also clears every wheel-finish setting.

A later visual test raised the separate test account to REP70, bought a raised
hood and taped headlights, and verified both on the car in free roam and after
returning to the garage. Both also survived a full game/server restart.
Category unlocks are separate inventory state from the displayed REP level;
the inventory load now grants eligible category bits from installed rules.
The front-rim test exposed an offer-filter bug: shared wheels can retain an
inactive scope list. That filter is corrected while retaining the current car's
active compatibility list. The retry bought and equipped the front rim for the
displayed price, retained the stock rear rim, and loaded the new front rim in
free roam and on garage return. That run also exposed a separate bug: changing
tires reset the rim paint and size because those save requests were unhandled.
The current candidate stages body paint, both rim paint layers on each axle,
and rim size in the same owned transaction path. Part and paint edits retain
request order; a later tire change preserves the saved rim finish. Portable
tests cover this regression, exact retries, cancellation, invalid requests and
database reopen. The follow-up live test confirmed the rim model after restart,
applied green primary and yellow secondary paint to both axles, increased the
front rim size, then replaced the front tire with an owned stock tire. The front
rim retained both colors and its size after the tire change, garage return and
a full game/server restart. The rear rim retained its green paint as well.
Both axles' green paint was visible in free roam; the yellow lip was difficult
to distinguish under the outdoor night lighting. The saved account retained
the exact paint values and size selection, with no further cash deduction.

Paint requests omit a vehicle reference and resolve against the active owned
garage car. Selection across multiple owned cars is covered by the completed
[car ownership and garage management issue][garage-mgmt].
Size requests must identify an installed owned rim and a size that fits its
authored axle dimensions.

The paint-library loading wait was caused by an unanswered speed-wall read on
an empty account. Supported reads now return the defined not-found error when
there is no saved row, allowing the client's shared download queue to continue.
A live test reached the library and body-paint editor with all header downloads
completed. The first save exposed the missing wrap upload handler.

The current candidate adds durable livery uploads, downloads, metadata reads
and deletion. It preserves complete client-generated payloads in the account
database and returns upload success only after commit. Partial uploads, foreign
owners, unsupported addresses, quota failures and storage errors cannot produce
a successful save. Portable tests cover fragmented HTTP, exact retries, reopen
and account isolation. A live test saved a body-paint wrap and a rear-window
decal wrap: all six header, recipe and baked-image payloads matched durable
storage byte for byte. A follow-up loaded both entries after restarting game and
server, applied the newer saved wrap, and committed its body paint and livery
together. Its rear-window stripe was visible in free roam and the body paint
survived garage return. A further restart retained the yellow body paint in the
garage and the rear-window stripe in free roam. These checks cover restoring
and applying a saved library entry. A later live test saved a newly edited green
wrap through the livery-field handler: all three uploaded payloads matched storage,
and the green body survived free roam, garage return and a game/server restart.
The newly added rear-body mark was not visually distinguishable, so that check
does not establish correct decal placement or recipe editing after reload.
It migrates storage to schema 5; back up the stopped account directory before
testing. Older server builds cannot open the upgraded database.

The follow-up candidate models livery edits and equip in the same transaction
as body paint. Library livery leaves retain their account ownership and item ID;
normal parts still require their car owner. The two livery fields and changed
flag are stored without inventing a resource hash. A library item referenced by
another car is currently refused to avoid changing that car through a shared
record. Portable tests cover combined edits, retries, cancellation, account
isolation, unrelated part changes and restart. The live library-apply path
retains the account-owned livery ID and its installed resource hash. The later
new-wrap save also exercised the request that changes the two livery fields.

These checks cover the tested starting car, not every supported definition or
menu path. The later visual and wrap checks below complete the recorded scope;
untested definitions and menus are not implied to work. Client
performance numbers changing immediately after purchase are only a local preview;
the return/restart checks are needed to establish that they were saved.

A combined owner test on 2026-10-15 changed tint, plate fittings/text, spoiler,
fenders, splitter, mirrors, body paint and stance, then saved a new wrap with a
large windshield decal. The owner confirmed the installed appearance survived
free roam and garage return. Discarding an extra decal and paint preview restored
the saved appearance; removing the plate fittings kept the accepted text; changing
the front tires preserved the finish. A subsequent agent test restarted both game
and server and confirmed the saved windshield decal in the garage and free roam,
the plate text and retained fitments, then returned to the garage. All three wrap
uploads still matched durable storage byte for byte. A later test on a separate
account copy reopened the saved rear-window stripe, moved it, changed body paint
and intentionally overwrote the same library entry. All three replacement uploads
matched durable storage. A full game/server restart then retained the green paint,
original windshield logo and moved rear-window stripe. Reopening the decal editor
confirmed the saved position and scale. The subsequent factory-paint reset also
passed free roam, garage return and restart, as described above.

Ride height and front/rear camber are saved on the installed control-arm tuning
item and included in the free-roam car's chassis customization. The owner's new
values survived restart, and the car appeared lowered outside. The early night
views were inconclusive; the later independent stance checks are recorded below.
Canard and caliper purchases initially failed because the projection excluded
all parts with nonempty fitment restrictions. The current candidate models those
restrictions at equip time. Follow-up live checks bought and fitted two compatible
canards and an Endless rear caliper, including both definitions that failed in the
owner's test. The first canard and caliper survived a garage/world/garage loop and
a game/server restart; both were marked currently equipped after restart. Buying
a conflicting canard as a spare deducted its price once, kept it owned but
restricted, and left the installed canard and finish unchanged. Saved records
retain the original body/wheel finish and stance through these changes. The owner
then confirmed the canards were clearly visible in free roam. Changing to rims
with larger openings and red calipers made the calipers clearly visible too;
the owner explicitly validated their outdoor appearance on 2026-10-15. These are
owner-reported visual passes for the tested car and combinations, not every
restriction edge or caliper definition. Together with the earlier transition,
restart and conflicting-fitment checks, they complete the acceptance recorded in
[issue #69](https://github.com/afterhours-crew/afterhours-2015/issues/69).
For twelve captured visual-part requests, complete purchase replies were sent
122–173 ms after request completion; successful purchases' subsequent inventory
updates were sent after 432–529 ms. These measurements do not establish how long
the visible loading indicator lasted. A later combined run timed a hood and two
front-rim purchases. Request-to-complete-reply intervals were 170, 120 and 145 ms;
inventory updates completed within 431–469 ms and were acknowledged within
470–518 ms. The UI had returned to the part editor by 1.40, 0.32 and 0.88 seconds
after confirmation, respectively. These are sampled upper bounds, not exact
spinner durations. All three purchases charged the catalog price once, saved the
selected part and retained the replaced parts as spares. The longer owner-reported
wait did not recur, so its cause remains unresolved; no timing fix or claim that
the original wait was normal follows from these measurements. This investigation is recorded in
[issue #70](https://github.com/afterhours-crew/afterhours-2015/issues/70).

The candidate supports plate text, window tint and restoring factory body paint.
Plate text uses the native seven-byte limit and terminator; tint retains finite
values from zero through one. Factory paint comes
from the current car's installed definition and preserves tint, plate, livery and
wheel finishes. Portable tests cover exact request bounds, cancellation, retries,
account isolation, stale edits, storage reopen and later equipment changes.
A later live test changed window tint and committed the edit; the slider retained
its setting after restarting
both game and server. The local utility text filter now allows valid UTF-8 text
without control characters, bounds requests and returns native result codes.
It makes no external calls and does not implement EA's moderation policy. The
game accepted a plate edit with this reply, but did not send a commit before the
next editor began; the plate reverted on entering free roam. The fix treats the
accepted plate RPC as a standalone submission through the existing validated
transaction queue. Paint and tint
previews still need their explicit commit; accepting a plate never saves them.
Tests cover ordered submissions, retry safety, canceled previews, queue bounds
and refusal of externally stale state. Live tests then retained the accepted text
through another part editor, free roam, garage return and a full game/server
restart. Resubmitting identical text did not advance the account generation.
This is a compatibility policy supported by the observed client sequence; the
original backend's internal save timing is not established. Factory-paint reset
has since passed; text-dialog cancellation also retained the saved text when
the editor was reopened after discarding a draft. The text-filter
prerequisite and its live result are
tracked in [issue #68](https://github.com/afterhours-crew/afterhours-2015/issues/68).
An earlier attempted combined run stopped at the splash because a Windows
firewall prompt from a completed test covered it.
The QoS rejection test now checks invalid addresses without binding wildcard
sockets. The remaining network integration tests use loopback; no firewall rule
or administrator access was added.

An activation wait also exposed a startup delay: the finite QoS listener expired
before the player reached the splash. The server now keeps QoS listeners alive
until shutdown, with four concurrent TLS workers and a separate deadline for each
request. Tests cover idle waits, repeated TLS and UDP requests, stalled clients
and prompt shutdown. The next live run completed startup requests in about three
seconds, reached the garage and closed normally. This run did not repeat the
long activation wait; that lifetime regression has automated coverage. See
[issue #67](https://github.com/afterhours-crew/afterhours-2015/issues/67).

The candidate also models removing the installed part in a selected class/location
and restoring stock visual parts. Removal retains the owned record and cash.
Visual reset restores stock appearance and tint, resets installed stance tuning,
and preserves performance, body paint, plate text and livery; its wheel option
retains rims, tires, calipers and brake discs. Body kits and plate fittings are
removed. Portable tests cover ownership, ordering, cancellation, retries, stale
edits and reopen, and an independent installed-state comparison passes. A live
plate-frame removal sent the modeled request and removed the installed child
without changing cash, plate text, tint or paint. Its appearance was unchanged
in free roam and after restart; inspection then showed that the selected frame
uses the same textures as the car's default. That test cannot distinguish a
correct reset from a rendering failure. The later owner test fitted a different
frame and background, removed them, and confirmed stock appearance with the new
plate text retained through garage return; the subsequent restart preserved it.
The first stock visual reset test exposed a request-width bug: the wheel option
is one bit, but the decoder required a byte. The local preview reset while the
server rejected the request, preventing the following garage exit from progressing.
The decoder is corrected, with portable tests for both options and the following
commit, plus an exact captured-request comparison. The first follow-up stopped
when screen capture failed during loading and did not establish persistence.
Later combined tests verified both reset options, following edits, garage exits
and the final state after full restart. The factory-paint reset also passed in
those later tests. See [issue #71](https://github.com/afterhours-crew/afterhours-2015/issues/71)
and [issue #66](https://github.com/afterhours-crew/afterhours-2015/issues/66).

Resetting all performance parts is now modeled separately from factory paint.
It restores the car's recorded performance defaults, retains visual parts and
purchased spares, and does not refund cash. A default part's saved tuning remains
attached to that item; this operation does not reset every handling slider.
Request ordering, cancellation, retries, account isolation, stale writes and
database reopen have automated coverage. A live reset restored the stock filter
and cooling parts, retained cash and visual customization, and survived a
garage/world/garage loop. A later reset from eight installed upgrades and nitrous
passed full game/server restart; stock equipment and dyno values persisted, and
the purchased parts remained owned spares.

A zero-cash live test displayed Insufficient Funds for an unowned sway-bar offer.
Attempting to select it left the account generation and inventory unchanged;
the normal client did not send a purchase. Purchase cancellation and discarding
an appearance preview also passed. These checks validate the normal UI paths;
server-side rejection of insufficient funds, retries and concurrent stale edits
has separate automated coverage.

Driving parameters are derived from equipped parts and their saved tuning when
the world car spawns; cached dyno figures are a separate display record. A server
regression saves an upgrade and handling changes, reloads inventory, and decodes
a fresh free-roam spawn to verify its performance contribution and tuning values.
Changing only the dyno cache leaves those chassis inputs unchanged. This verifies
the server's driving inputs. A live transmission comparison also confirmed that
Manual Gears holds first gear under throttle while automatic shifts into second;
the manual behavior was also observed after restarting game and server.
The short stock-versus-supercharger acceleration comparison was inconclusive:
wheelspin and launch control were not controlled. The owner's later driving
comparison validates a noticeable upgrade effect qualitatively. It does not
establish measured acceleration figures. The additional handling and stance
checks below distinguish observed effects from contract coverage.

REP-driven performance unlocks are separate from the visual category mask.
The candidate reads explicit item-unlock branches from the installed progression
graph and creates durable purchasable records when their REP score is reached.
Buying still charges the installed price. Selecting a purchasable inventory item
promotes that same item ID to car-owned; selecting a definition creates a new
record. Retries cannot duplicate the grant or charge. Unconsumed grants remain
available after an operator lowers REP. Debug
unlock-all and story-award branches are excluded. Item content version 6 and a
new builder revision rebuild existing caches automatically. Portable tests and
an independent installed-content comparison pass. Live testing unlocked the
previously locked SC-V2 and cancellation retained the balance. Two failures were
corrected: returning a new ID for an item-ID purchase, and rejecting its in-place
promotion when refreshing the live inventory. The follow-up purchased and equipped
SC-V2 for 10,900, retained the remaining 82,330 balance and existing appearance,
then passed free roam, garage return and a full game/server restart. The restarted
garage retained SC-V2, the balance and saved dyno values; see [issue #63][rep-item-issue].

The client loads Items once per session, so a level-up's grants are announced
when the credit commits. The server compares the account's item collection
before and after the credit and sends, one transfer at a time on the Items
handler, a change notification per new record (sequence 1: a state byte, the
runtime player index, then the added and the removed collections), followed by
an update notification per changed root (the category unlock controller). The
client builds the added records as new items and inserts them into its live
inventory (receiver `0x14406f9e0`, insert `0x14407d0c0`, erase `0x14408ae70`
in the supported build). An official host sent the same notifications at a
level-up, one record each about 0.3 s apart, and both codecs reproduce those
bytes exactly. Live, a level-up to 3 made both level-3 tire compositions
buyable in the garage without a restart, and one was bought and fitted. Buying
promotes the offer. Its replacement now commits in the same purchase transaction
and receives a separate change notification immediately, without another REP
credit or load. Retries retain the purchased identity, single charge and one
replacement. Removals are never announced by these grant paths.

Item content version 9 also reads the installed REP award branches: wraps at
levels 51–60 and decal packs at 61–70. These ownerless owned rewards use the
same allocator and transaction as REP and part offers. Existing owned rewards
prevent repeat grants, including after restart. Older content formats never
infer rewards. The decal seen alongside the earlier level-3 crossing was a
daily-challenge reward paired with REP, not a level-3 unlock; challenge
progression remains under [#57](https://github.com/afterhours-crew/afterhours-2015/issues/57).
These paths passed live acceptance on 2026-10-18. A copied test account prepared
one point below level 61 earned the crossing, received the Speedhunters heart
decal and let the owner select/apply it in the editor. The account held each of
the ten eligible wraps once and no level-62–70 decal. Buying a $215 level-3 tire
debited exactly $215 and delivered a fresh offer 200 ms after the purchase reply,
without another REP credit. Restart retained the reward and purchased/replacement
identities without duplicates; the owner confirmed parts and decal remained.
Preparation is not evidence of earning every preceding level, and individual
wrap presentation was not exhaustively checked. See [#92](https://github.com/afterhours-crew/afterhours-2015/issues/92).

For unattended menu testing, [change the game's Back key to F9](MENU-KEYBINDING.md)
through its user profile. This works without a mod and leaves Computer Use's
Escape stop control available.

## Build and start

Follow [the local launcher guide](LOCAL-LAUNCHER.md) for Windows prerequisites,
the supported installation, initial setup and activation requirements. Build
the current source, including the changed server:

```powershell
cargo build --release --workspace --locked
$data = Join-Path $env:LOCALAPPDATA 'Afterhours2015-Upgrades'
.\target\release\nfs-afterhours.exe start `
  --game-dir 'F:\EA Games\Need for Speed' --data-dir $data
```

The default server path is the build beside the launcher. A deliberate debug test can use
`--server .\target\debug\nfs-server.exe`. Content is rebuilt automatically
when the builder revision changes. Existing accounts are preserved. No private
scripts, captured replies or administrator access are required.

## Give a local test account cash

Earning money from races is not implemented. Accounts without a bank record
start with zero cash. The explicit operator command below sets test cash; it
does not record race earnings or prove a reward loop. Use a separate test data
directory when experimenting. Back up its account directory while stopped.

Stop the game/server before inspecting and changing the balance:

```powershell
.\target\release\nfs-afterhours.exe stop --data-dir $data
```

Wait for the original launcher window to report shutdown complete. Then inspect:

```powershell
$account = (Get-Content -LiteralPath (Join-Path $data 'account.json') -Raw |
  ConvertFrom-Json).local_account
$state = Join-Path $data 'artifacts\account'
$bank = (& .\target\release\nfs-server.exe bank --state-directory $state `
  --local-account $account | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw 'Bank inspection failed' }
$bank
```

Choose the amount deliberately. This example sets the total balance to 2,500:

```powershell
& .\target\release\nfs-server.exe bank --state-directory $state `
  --local-account $account --set 2500 --expected-generation $bank.generation
if ($LASTEXITCODE -ne 0) { throw 'Bank adjustment failed; inspect before retrying' }
```

The command requires an existing account and the inspected database generation.
Stale writes fail; it does not automatically retry a changed account. Inspection
does not create an account. `--set 0` with the current generation removes the
test balance. The JSON output reports old/new balance and generation. Restart
the game/server to load the new balance.

## Give a local test account REP

For server development, `nfs_services::reputation::earned::Policy::prepare`
builds a single generation-guarded storage batch containing a calculated REP
credit, its derived level, category masks and newly eligible purchasable items.
It leaves unrelated tables, money and equipment unchanged. The caller owns the
scoring decision and retry ID; retain the original batch and generation on a
retry, and include any additional reward in that same transaction. Never rebase
an acknowledged credit onto a later snapshot. Repository replay history is
bounded, so the original generation is also necessary to reject expired retries.

This domain API is covered by constructed transaction and SQLite restart tests.
The free-roam scoring interpreter now supplies driving credits. Live checks
covered earning REP, crossing levels 2 and 3, same-session part availability
and purchase, and level-2 REP/unlock persistence after restarting game/server.
The owner accepted the remaining action and popup checks on 2026-10-18; see
[#50](https://github.com/Afterhours-Crew/afterhours-2015/issues/50) and
[driving-score calculations](SCORING.md) for the tested scope and limits.
The operator command below remains a separate stopped-account testing tool.

Use a separate test account, stop its game and server, and back up the account
directory as above. REP is calculated from total score; changing only a saved
level is insufficient. The operator command sets the total score to the
installed threshold for a chosen level. It preserves cash, inventory, style
scores and other progression; this is a test grant, not earned progression.

Resolve the content paths for your installation, then inspect the account:

```powershell
$content = (& .\target\release\nfs-server.exe prepare-content `
  --game-dir 'F:\EA Games\Need for Speed' --root $data | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw 'Content preparation failed' }
$repArgs = @('reputation', '--state-directory', $state, '--local-account', $account,
  '--persistent-content', $content.files.persistent_content,
  '--reputation-thresholds', $content.files.reputation_thresholds)
$rep = (& .\target\release\nfs-server.exe @repArgs | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw 'REP inspection failed' }
$rep
```

Set the highest level defined by the installation:

```powershell
& .\target\release\nfs-server.exe @repArgs --set-level max `
  --expected-generation $rep.generation
if ($LASTEXITCODE -ne 0) { throw 'REP adjustment failed; inspect before retrying' }
```

Restart to load the new score. The JSON report shows the old/new score, level
and generation. `--set-level 1` with a freshly inspected generation resets REP
to the first level threshold. `--set-score <total>` (instead of `--set-level`)
stores an exact total and derives the level from the installed thresholds, for
example a total a few thousand points under the next threshold so a live
drive reaches the next level. On inventory load, authored REP rules add eligible
visual-category unlocks to the account's category controller. Existing unlocks
remain unlocked if REP is later lowered. Restore the stopped account backup to
recover the earlier category state and exact score within its level. A stale generation fails without
rebasing the adjustment. High REP does not override compatibility or separate
story/build requirements, nor does it implement currently unsupported parts.

## Validation scope

The final tuning checks on 2026-10-15 exercised the available handling rows after
fitting their required parts: tire pressures, steering response/range, braking
and drift controls, differential, handbrake, sway bars, downforce, nitrous tuning
and transmission. Saved values survived garage return and full game/server
restarts without changing unrelated equipment, appearance or cash. The installed
extreme-grip tire fixed its traction setting; spring stiffness remained locked
behind Amy's story progression. Those are client restrictions, not missing saves.
The owner deferred story/build unlocks to [#56](https://github.com/Afterhours-Crew/afterhours-2015/issues/56)
through [#75](https://github.com/Afterhours-Crew/afterhours-2015/issues/75).

The six exposed stance controls are height, rake, front/rear track and front/rear
camber. A lowered configuration with negative front and neutral rear camber was
validated outdoors by the owner. The reverse configuration retained neutral front
and negative rear camber after restart; the later wider front/rear camera views
showed the separate wheel angles outside. These are qualitative visual checks,
not measurements of degrees or suspension travel. Caster and toe have native data
fields but were not exposed by the tested stance menu.

Separate checks load the two saved configurations through the canonical garage
and driving-car constructors and round-trip their wire encoding. The creation
contains the saved performance contributions and all 32 serialized tuning slots;
an accompanying motion update may omit unchanged customization. This establishes
the server's active-car inputs, not independently measured physics for every
slider. Direct driving acceptance remains the owner's grip/upgrade comparisons
and the observed manual-versus-automatic transmission behavior.

Purchase recovery was tested with two seconds of incoming packet loss during a
fuel-pump purchase. The client retransmitted the file-transfer start; the purchase
and equip completed, charged the catalog price once and survived a game/server
restart. The first attempt exposed an acknowledgement-window bug. The corrected
sender reuses the current empty-control sequence under pressure and suppresses
timing traffic that would exceed the receipt window. It retains the strict
32-position acknowledgement bound. A regression covers extended acknowledgement
loss and sequence wrap; the live pass covers the stated two-second case, not
arbitrary outage lengths. See [#80](https://github.com/Afterhours-Crew/afterhours-2015/issues/80)
and the opt-in [packet-loss diagnostic](../CONTRIBUTING.md#local-packet-loss-diagnostics).

The ordinary purchased-spare menus expose no Sell action in the tested client.
The conditional selling investigation [#76](https://github.com/Afterhours-Crew/afterhours-2015/issues/76)
is resolved on that basis; selling is not implemented by this milestone. The
actual whole-car and claimable-reward selling paths, including their credit and
deletion rules, remain explicit work in [garage management][garage-mgmt] and
[progression/inventory](https://github.com/Afterhours-Crew/afterhours-2015/issues/53).
Nested kits and kits containing library liveries also remain unsupported.

Final runtime validation passed formatting, workspace Clippy and 1,262 tests,
plus the optional network-test feature checks. Raw reference evidence stays in
the separate research repository; this checkout builds and runs independently.

The nitrous transition handler acknowledges the authoritative driver's start
and stop requests through the current car's nitrous mode. The client waits for
this update before sending its next transition; omitting it can leave exhaust
flames active whenever the accelerator is pressed after the first nitrous use.
The handler resolves the component selector from the live creation, checks
connection/persona ownership and world-car readiness, and queues its update
atomically with the vehicle state. It does not replace the fuel level or infer
nitrous use from throttle. Portable checks cover start/stop/repeats, malformed
requests, foreign owners, replacement lifetimes and output-queue rollback.
A fresh game/server run on 2026-10-15 validated accelerator alone, repeated
nitrous use, release of nitrous while retaining the accelerator, and release of
both controls. Blue jets appeared during nitrous use and disappeared after
release even while the accelerator stayed held. Subsequent acceleration alone
did not restore them. Gauge depletion and time refill remained functional with
the tested Time Refill NOS 5 lb kit. The recorded client sent alternating start
and stop requests, where the failing run had sent only the first start. The
owner also reported the correction working. See
[the exhaust-effect issue](https://github.com/Afterhours-Crew/afterhours-2015/issues/77).

Purchase initializes the part's typed defaults from its installed definition,
inserts the owned part, deducts cash and records a durable receipt in
one transaction. Retrying that offer for that car returns the original part
without charging again. Equip stages an account-bound edit; commit replaces the
installed child of the same class/location while retaining the displaced part
and stock defaults. Multiple categories accumulate in one edit; a later selection
of the same category replaces its earlier preview. An edit accepts at most 128
selections and saves all of them together.
Restoring stock is an equip operation and does not refund cash. Cancellation
before purchase confirmation does not send a purchase. Merely staging an edit
does not write storage.

The candidate also accepts the client's bounded dyno-result message for the
active garage car. Its seven performance fields can commit with the selected
part, or as a separate result for the current inventory generation. These are
client-calculated cached figures; the server does not invent them or simulate a
dyno. Paint, license plate, unrelated vehicle flags, other items and cash are preserved.
After a successful commit the server sends an inventory update on the Items
File channel. Delivery must finish before the next account write starts, so a
later edit cannot replace a pending update.

Incoming MessageEX chunks are reassembled before dispatch, retaining the order
of equip, dyno and commit even when a result spans handler frames. Reliable
message retries do not repeat a committed edit. Each connection/channel has a
bounded assembly buffer and a deadline; gaps or incomplete mutations cannot be
silently skipped before processing their commit.

Handling requests accept up to 31 distinct category/value pairs with slider
values from 0 through 10. Repeated requests in one edit keep the latest value
for each category. The server resolves the current installed part in child
order, excluding stock defaults and unequipped spares. Transmission mode uses
the installed persistent-tuning asset's values and preserves other car flags.
All changed part records and any accompanying equip/dyno result save atomically;
the inventory notification and vehicle equipment are refreshed from that same state.
Beginning another edit cancels the pending changes. Missing or unsupported
installed targets refuse the save instead of changing an unrelated item.

The version 7 item catalog includes complete front/rear locations for spatial
parts, the car's compatibility list and minimum REP scores for every offer.
It adds the installed definition-to-definition fitment restrictions to the earlier
catalog formats. Content preparation rebuilds caches produced before this policy;
older explicit catalog versions retain their original offer policy. Restriction
rows require known definitions, unique targets and bounded counts. The server
checks fitment when equipping, using the ordered pending edit. Purchasing a part
does not install it; conflicting parts remain owned spares until they can be fitted.
Completed purchase receipts still replay if equipment changes later.
Unknown unlock conditions are excluded. REP is checked against the same committed
account generation as the purchase debit; changing a displayed level cannot
authorize a purchase. An existing purchase receipt remains retry-safe after an
operator lowers REP. Owned parts and stock defaults can still be equipped.
Visual commits prevalidate chassis customization, wheel geometry, appearance and
mesh encoding before writing inventory. Newly needed mesh registrations are
queued before the car update; motion, authority and identity remain unchanged.
Fresh garage and world spawns also register selected-part bundles before the
vehicle creation, so a previous session's mesh cache is not required.

Portable tests cover malformed/truncated requests, invalid owner references,
insufficient funds, repeated purchases/commits, competing edits, account
isolation, SQLite reopen and storage failure. They also check that a car update
keeps its motion/identity and that refreshed inventory allows a later garage
return. All tests use constructed inputs and local temporary state.
Dyno tests cover variable-length float fields, exact payload extents, cancellation,
combined part/result commits, stale generations and preservation of unrelated
vehicle data. Inventory update tests cover record closure, resource bounds and
delivery ordering. These checks do not establish that the game sends dyno results
in every menu path; that remains part of live acceptance.
Broader performance tests cover nonempty typed defaults and independent receipts,
ordered multi-category edits, stock restoration and tuning the final selected part.
The generated offer set and prices are compared with the supported installation;
air-filter and cooling upgrades have live purchase/equip/return/restart coverage.
Handling checks cover all category layouts, malformed and duplicate settings,
combined part changes, transmission selection, cancellation, stale edits,
account isolation, notification/chassis refresh and SQLite restart persistence.

The runtime validates the active player's garage-car binding before accepting
an edit. Unsupported or failed commit paths do not send invented success replies;
currently an unrecoverable commit/storage conflict ends that world connection.
Reconnect to load the committed account state. Live acceptance of conflict and
storage-failure recovery is distinct: the stale-edit conflict has now passed a
live check, while arbitrary disk/storage failures have portable coverage only.
During a running garage edit, an operator update advanced the same account's
generation. Saving the stale edit rejected it and disconnected the world.
Restart retained the newer cash balance and discarded the unsaved tuning preview.
A fresh tuning edit then saved successfully and was visible on reopening the menu.

A separate fresh account bought an air filter and cooling upgrade for exactly
their displayed prices. Another account database in the same state directory
remained byte-identical. Restarting game and server retained both installed parts,
the exact balance and cached dyno values; the account's generation and item records
were unchanged by that restart. A four-second pause of the server delayed one
purchase but it completed after resume. The capture contained no repeated purchase
fragment, so that pause is not evidence of retransmission handling.

The plate text editor's cancel path was also checked separately: deleting the last
character and canceling restored the original text when the dialog was reopened.
See [menu key bindings](MENU-KEYBINDING.md) for the native text-dialog exception to
the normal Back binding. The local filter does not contact an external moderation
service or claim to reproduce its policy.

For purchase timing, twelve sampled visual purchases completed their server
replies in 122–173 ms. A combined hood/rim run measured purchase replies of
170/120/145 ms, inventory delivery in 431–469 ms and acknowledgement in 470–518 ms;
the first observed completed UI frames bounded the visible waits to at most
1.40/0.32/0.88 seconds respectively. Durable commits took 15–24 ms. A later six-part
batch completed replies in 137–171 ms with the exact total debit and retained
customization. These observations did not reproduce the earlier reported roughly
ten-second waits. The captures distinguish server reply and inventory delivery
from later client presentation; they do not isolate asset loading or provide an
original-backend baseline. No claim that the ten-second delay was normal or fixed
follows from these measurements.

[garage-issue]: https://github.com/afterhours-crew/afterhours-2015/issues/49
[garage-mgmt]: https://github.com/afterhours-crew/afterhours-2015/issues/52
[rep-item-issue]: https://github.com/afterhours-crew/afterhours-2015/issues/63
