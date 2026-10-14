# Garage upgrade development

This candidate models unrestricted, REP-zero leaf performance-part purchases,
their equip/save requests, and restoring stock parts. The install-content builder
selects supported offers from the installed definitions. Prices, part GUIDs and
owned item IDs are never copied from a saved account or hardcoded in the server.
The candidate also stages the 31 handling controls against installed parts and
the car's transmission flag. Parts with REP or other restrictions, composite
parts, visual customization, selling and wraps remain unsupported.
See the [gameplay checklist](GAMEPLAY-CHECKLIST.md).

The purchase, cash deduction and durable equip have been accepted by the game.
The latest combined live test restored stock, re-equipped the upgrade, drove and
returned to the garage with the correct filter label and unchanged cash. The
performance figures still reverted to the initial values. A subsequent saved
traffic comparison found the dyno result inside split world messages; the server
now assembles those messages before processing the save. This fix and handling
changes have passed automated checks but await combined live validation and
restart testing. This candidate is incomplete and must
not be treated as a working upgrade feature. Client
performance numbers changing immediately after purchase are a local preview;
they alone do not prove that the server saved or applied the part.

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

## Validation scope

Purchase initializes the part's typed defaults from its installed definition,
inserts the owned part, deducts cash and records a durable receipt in
one transaction. Retrying that offer for that car returns the original part
without charging again. Equip stages an account-bound edit; commit replaces the
installed child of the same performance class while retaining the displaced part
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
the inventory notification and chassis tuning are refreshed from that same state.
Beginning another edit cancels the pending changes. Missing or unsupported
installed targets refuse the save instead of changing an unrelated item.

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
only the original air-filter purchase has live acceptance so far.
Handling checks cover all category layouts, malformed and duplicate settings,
combined part changes, transmission selection, cancellation, stale edits,
account isolation, notification/chassis refresh and SQLite restart persistence.

The runtime validates the active player's garage-car binding before accepting
an edit. Unsupported or failed commit paths do not send invented success replies;
currently an unrecoverable commit/storage conflict ends that world connection.
Reconnect to load the committed account state. Live acceptance of these recovery
paths and the complete purchase/equip/drive/return/restart sequence remains open.
