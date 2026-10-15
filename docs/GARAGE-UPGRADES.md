# Garage upgrade development

This candidate models supported leaf performance and visual-part purchases,
their equip/save requests, and restoring stock parts. The install-content builder
selects supported offers from the installed definitions. Prices, part GUIDs and
owned item IDs are never copied from a saved account or hardcoded in the server.
The car's own installed compatibility list restricts both purchase and equip;
an offer for a different car cannot be bought or fitted just because it exists
in the catalog. Recorded stock defaults remain available to restore.
The candidate also stages the 31 handling controls against installed parts and
the car's transmission flag. REP-only offers use the account's saved score and
installed thresholds. Visual parts match exact class and, where applicable,
front/rear location. Composite parts, separate story/build restrictions and selling
remain unsupported. Saved wraps can be restored from the local library and
equipped. The live checks below cover selected performance
and visual parts; broader customization acceptance is still pending.
Remaining work and live acceptance are tracked in the
[garage upgrades issue][garage-issue].

The combined live test on 2026-10-14 equipped an owned air filter and purchased
a compatible cooling upgrade in one performance edit. Both parts, the exact
cash deduction and client-calculated dyno figures survived a garage/world/garage
loop and a full game-and-server restart. Front tire pressure, rear tire pressure
and steering response also survived both transitions. Earlier runs exercised
stock restoration and driving with the filter. The split-message receiver fix
resolved the previously lost dyno cache.

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

Paint requests omit a vehicle reference. For now they require one unambiguous
owned garage car; active-car selection across multiple owned cars belongs to
the separate [car ownership and garage management issue][garage-mgmt].
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
menu path. Broader visual parts and the complete wrap-edit workflow remain unfinished. Client
performance numbers changing immediately after purchase are only a local preview;
the return/restart checks are needed to establish that they were saved.

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
and text-dialog cancellation still need live acceptance. The text-filter
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
correct reset from a rendering failure. A distinct frame needs testing. Stock visual
reset remains untested through the game UI; see
[issue #66](https://github.com/afterhours-crew/afterhours-2015/issues/66).

Resetting all performance parts is now modeled separately from factory paint.
It restores the car's recorded performance defaults, retains visual parts and
purchased spares, and does not refund cash. A default part's saved tuning remains
attached to that item; this operation does not reset every handling slider.
Request ordering, cancellation, retries, account isolation, stale writes and
database reopen have automated coverage. A live reset restored the stock filter
and cooling parts, retained cash and visual customization, and survived a
garage/world/garage loop. Restart after this reset still needs a live check.

Driving parameters are derived from equipped parts and their saved tuning when
the world car spawns; cached dyno figures are a separate display record. A server
regression saves an upgrade and handling changes, reloads inventory, and decodes
a fresh free-roam spawn to verify its performance contribution and tuning values.
Changing only the dyno cache leaves those chassis inputs unchanged. This verifies
the server's driving inputs. A live transmission comparison also confirmed that
Manual Gears holds first gear under throttle while automatic shifts into second;
the manual behavior was also observed after restarting game and server.
The short stock-versus-supercharger acceleration comparison was inconclusive:
wheelspin and launch control were not controlled. A clearer performance comparison
and broader handling checks remain pending; menu numbers alone do not prove them.

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
Formatting, workspace Clippy and all 1,216 tests
pass, including a regression for the live inventory refresh and bank notification.

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
pwsh -NoProfile -File .\launcher\Start-Afterhours.ps1 -Action Start `
  -GameDirectory 'F:\EA Games\Need for Speed' -DataDirectory $data
```

The default server path is the release build. A deliberate debug test can use
`-ServerPath .\target\debug\nfs-server.exe`. Content is rebuilt automatically
when the builder revision changes. Existing accounts are preserved. No private
scripts, captured replies or administrator access are required.

## Give a local test account cash

Earning money from races is not implemented. Accounts without a bank record
start with zero cash. The explicit operator command below sets test cash; it
does not record race earnings or prove a reward loop. Use a separate test data
directory when experimenting. Back up its account directory while stopped.

Stop the game/server before inspecting and changing the balance:

```powershell
pwsh -NoProfile -File .\launcher\Start-Afterhours.ps1 -Action Stop -DataDirectory $data
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
to the first level threshold. On inventory load, authored REP rules add eligible
visual-category unlocks to the account's category controller. Existing unlocks
remain unlocked if REP is later lowered. Restore the stopped account backup to
recover the earlier category state and exact score within its level. A stale generation fails without
rebasing the adjustment. High REP does not override compatibility or separate
story/build requirements, nor does it implement currently unsupported parts.

## Validation scope

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

The version 5 item catalog includes complete front/rear locations for spatial
parts, the car's compatibility list and minimum REP scores for every offer.
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
storage-failure recovery paths remains open.

[garage-issue]: https://github.com/afterhours-crew/afterhours-2015/issues/49
[garage-mgmt]: https://github.com/afterhours-crew/afterhours-2015/issues/52
[rep-item-issue]: https://github.com/afterhours-crew/afterhours-2015/issues/63
