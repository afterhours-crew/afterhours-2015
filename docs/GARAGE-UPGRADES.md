# Garage upgrade development

This candidate models one unrestricted, REP-zero air-filter purchase, its equip
and save requests, and restoring the stock filter. The install-content builder
selects supported offers from the installed definitions. Prices, part GUIDs and
owned item IDs are never copied from a saved account or hardcoded in the server.
Other performance parts, visual customization, selling, handling and wraps remain
unsupported. See the [gameplay checklist](GAMEPLAY-CHECKLIST.md).

The purchase and cash deduction have been accepted by the game. The new equip
and save path passes portable tests and still needs live driving, garage return
and a game/server restart before this feature can be marked complete. Client
performance numbers changing immediately after purchase are a local preview;
they alone do not prove that the server saved or applied the part.

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

Purchase inserts the owned part, deducts cash and records a durable receipt in
one transaction. Retrying that offer for that car returns the original part
without charging again. Equip stages an account-bound edit; commit replaces the
installed air-filter child while retaining the displaced part and stock defaults.
Restoring stock is an equip operation and does not refund cash. Cancellation
before purchase confirmation does not send a purchase. Merely staging an edit
does not write storage.

Portable tests cover malformed/truncated requests, invalid owner references,
insufficient funds, repeated purchases/commits, competing edits, account
isolation, SQLite reopen and storage failure. They also check that a car update
keeps its motion/identity and that refreshed inventory allows a later garage
return. All tests use constructed inputs and local temporary state.

The runtime validates the active player's garage-car binding before accepting
an edit. Unsupported or failed commit paths do not send invented success replies;
currently an unrecoverable commit/storage conflict ends that world connection.
Reconnect to load the committed account state. Live acceptance of these recovery
paths and the complete purchase/equip/drive/return/restart sequence remains open.
