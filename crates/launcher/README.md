# nfs-afterhours

Command-line launcher for the Afterhours 2015 local server and, on Windows,
the installed game. It replaces the PowerShell and C# scripts that used to
live under `launcher/`.

The sibling `nfs-afterhours-bundle` package links this launcher and `nfs-server`
into one terminal executable, `Afterhours.exe`. Its server children use the
explicit `--afterhours-server` dispatch flag, recorded as `bundled_server` in
the session request. The same supervisor owns preparation, activation waits,
temporary routing and cleanup. Missing `bundled_server` defaults to false for
existing records; an explicit `--server` uses the standalone-server contract.

Preparation progress goes to stderr and names actual extraction, verification,
cache reuse, account and service steps. JSON results on stdout remain parseable.

```text
nfs-afterhours check   --game-dir <DIR>     verify installation, paths, registration; change nothing
nfs-afterhours setup   --game-dir <DIR>     prepare content and the local account; start nothing
nfs-afterhours serve   --game-dir <DIR>     run the server (every platform); on Windows `play` can join
nfs-afterhours start   --game-dir <DIR>     run the server, start and route the game (Windows)
nfs-afterhours status                       report the current session
nfs-afterhours stop                         ask the live session to close the game, then the server
nfs-afterhours play                         ask the running serve session to start the game (Windows)
nfs-afterhours api                          show how to reach the running session's API (--api)
nfs-afterhours profile [--account-id <ID>]  show a saved driver's profile; change nothing
                       [--game-dir <DIR>]   (car names come from the installed game,
                       [--language <LOCALE>] read-only, in the game's locale)
nfs-afterhours profile --list               list every saved driver in the data directory
nfs-afterhours car-images [--game-dir <DIR>]  decode car pictures from the game into the data folder
nfs-afterhours map-images [--game-dir <DIR>]  decode the world map from the game into the data folder
nfs-afterhours sessions [--limit N] [--id <ID>]  summarize session folders and logs; change nothing
nfs-afterhours settings [--traffic <PERCENT>] [--rivals <COUNT|original>]
                                            show or save traffic and rivals for later sessions
nfs-afterhours version                      program, JSON record versions and capabilities
nfs-afterhours schema <RECORD>              print the JSON Schema of one --json record
nfs-afterhours recover --account-id <ID>    restore account.json for an existing database
```

Every subcommand accepts `--data-dir <DIR>` and `--json`. `--help` on any
subcommand explains it. For installation requirements and the full
walkthrough see [docs/LOCAL-LAUNCHER.md](../../docs/LOCAL-LAUNCHER.md).

Progress and child diagnostics go to stderr; command results go to stdout.
With `--json`, a completed session prints one final status object on stdout,
including `interrupted` if its supervisor died. Errors still have a nonzero
exit code and a message on stderr. Observe an active session with `--events`
on the command that starts it, or with a separate `status --json` command.
Recorded process IDs may be stale; verify process identity before stopping an
orphan after a Unix supervisor crash.

## JSON contract

A desktop front end (the Tauri launcher) uses this program only through its
JSON output. Every object it prints is a *record* with a name, a `version`
field and a JSON Schema (draft 7) generated from the Rust type that prints it:

| Record | Printed by |
| --- | --- |
| `check` | `check --json` |
| `status` | `status --json`: `{"version", "session"}`, `session` `null` before any session |
| `stop` | `stop --json`: `outcome` `requested` or `already-over`, with the status |
| `play` | `play --json`: the status when the request was written and the request file |
| `session-status` | `setup`/`serve`/`start --json` (the final status); also `status.json`. It carries the session's `action` and `game_running`, so `serve` without a game is hosting and with one is playing |
| `event` | each line of `setup`/`serve`/`start --events`, including the live map positions (`map-feed`); also each event of the session API's `/v1/events` |
| `api` | `api --json` and `api.json` in a session started with `--api`: the session ID, the API's address and its token |
| `api-error` | the body of every refused session API request |
| `profile`, `profile-list` | `profile --json` (also the session API's `/v1/profile`), `profile --list --json` |
| `car-images` | `car-images --json`: the image set, counts per size, formats and unresolved images |
| `map-images` | `map-images --json`: the tile set, its detail levels, file naming, the world box it covers and any missing tiles |
| `sessions` | `sessions --json` |
| `settings` | `settings --json`: saved `traffic_percent` and `rival_limit`, the largest accepted values, and whether this command saved them; also the session API's `/v1/settings` |
| `recover` | `recover --json` |
| `version` | `version --json` |

The schemas are committed in [`schema/`](schema/), printed by `schema
<record>` and shipped with every release, inside the bundle and as
`Afterhours-contract.zip` next to `version.json`. `contract::tests` fails when
a committed schema differs from its type; regenerate with
`AFTERHOURS_UPDATE_SCHEMAS=1 cargo test -p nfs-afterhours --lib contract` and
review the diff. Unit tests validate real records against the schemas with a
strict checker that rejects any property a schema does not describe.

`version --json` is the record to read first: program (`Afterhours` or
`nfs-afterhours`), package version, release tag, source revision (`null` for
local builds), whether this platform can launch the game, the current version
of every record, and `capabilities`: `profile`, `sessions`, `events`, `play`,
`car-images`, `map-images`, `world-settings` (the `settings` command),
`map-feed` (live positions in the event stream) and `api` (the session API)
today. A screen whose capability is missing should say the feature is not
available yet.

Versioning: until the desktop launcher's first release, every record stays at
version 1 and may change in lockstep with the launcher. From that release on,
removing or renaming a field, changing its type or meaning, or removing an
enum value bumps that record's version; adding a field or an enum value does
not, so readers must ignore unknown properties. Errors are not records: they
are the exit code plus a message on stderr (in event mode, the `final` line).

### Event stream

`setup`, `serve` and `start` with `--events` print JSON lines on stdout
(issue #86). Each line is an `event` record with `version`, `utc` and `event`:

- `session`: the session ID, action, data directory and session directory,
  once the session directory exists and the supervisor starts.
- `status`: every state the supervisor records, exactly as `status.json`
  holds it (`preparing`, `starting`, `activation`, `ready`, `stopping`,
  `finished` or `failed`).
- `diagnostic`: one line a child wrote to stderr, with `source` `prepare`,
  `create-account`, `server` or `game`. Lines over 16 KiB are split; if the
  supervisor falls behind, a line saying output was skipped stands in for it.
- `map-feed`: where the cars are, at most four times a second while the
  server runs a world session (see [Map feed](#map-feed)).
- `final`: always the last line, also when preflight refused the command
  before a session existed: `exit_code`, `message` (`null` on success) and the
  final `status` (`null` if none was recorded).

The front-end process writes `session` and `final`; the supervisor it starts
inherits stdout and writes `status`, `diagnostic` and `map-feed`, so the lines
never interleave. Human console text is silenced in this mode. A failed write, for
example after the reader closed the pipe, disables further events without
stopping the session: the supervisor outlives a closed launcher window, and
`status --json` and `sessions --json` still observe it. A supervisor that is
refused before its session begins (another session holds the data directory,
a stop arrived, the server changed) records that reason as a `failed` status.

### Map feed

With `--events`, the supervisor starts the server with `--map-feed`, and
`serve` and `start` report where the cars are for a map view (capability
`map-feed`). It is read-only: the server observes the state its world
sessions already own and changes nothing. Positions come from the cars'
replicated chassis state (the game's latest report for cars it drives) or,
for a roaming rival without a car, from the server's own simulation of it on
the road network.

```json
{"version":1,"utc":"...","event":"map-feed","frame":42,"skipped":0,"worlds":[
  {"connection":1,"persona_id":"1000000007","world_ms":81250,
   "drivers":[{"participant":1,"x":-1234.57,"y":12.35,"z":6543.21,"heading":90.5,"speed":12.35}],
   "traffic":[{"id":17,"x":-1180.0,"y":11.9,"z":6610.25,"heading":182.3}],
   "rivals":[{"id":3,"x":-900.5,"y":10.0,"z":6800.0,"heading":null}],
   "traffic_ring":{"inner":300.0,"outer":450.0},"omitted":0}]}
```

- `worlds`: one entry per world session (one per connected game), ordered by
  `connection`. A frame with no worlds means none is running: remove any
  markers. In a local session every driver is the local player; `persona_id`
  matches `profile`'s `persona_id`.
- `drivers` are the players' cars driving in the world (garage and menus show
  none), ordered by `participant`; `traffic` and `rivals` are ordered by `id`,
  which is unique within its list for that world session and never reused
  there, so markers can be keyed by connection, kind and id.
- Units and axes: x, y and z are world metres (y up), rounded to centimetres.
  On the `map-images` map +x points right and +z points down, so a position
  is at u = (x - min_x) / (max_x - min_x), v = (z - min_z) / (max_z - min_z)
  of its world box. `heading` is the car's forward direction in degrees
  clockwise from up the map (0 up, 90 right), `0 <= heading < 360`, to a
  tenth, or `null` when it is unknown or vertical. `speed` is metres per
  second.
- `traffic_ring` gives the fixed radii of the native traffic policy, in
  horizontal metres around each driver: cars are created beyond `inner` and
  up to a little more than `outer` ahead of where the driver will be in about
  three seconds (a quarter of those distances to the sides), and kept while
  within `outer`. It is `null` without traffic or while traffic is off. A
  circle at these radii is an approximation of that shape.
- `omitted` counts cars the server left out of the frame (no usable
  position, state it could not read, or beyond a bound).
- Rate and order: each world session is observed at most every 250 ms, after
  its tick's traffic and rival updates, and the server writes a frame when
  anything changed, at most every 250 ms. The supervisor emits the newest
  frame on each of its 250 ms polls if it is newer than the last one sent, so
  `frame` only increases and `skipped` counts the frames in between that were
  not delivered (superseded before a poll, or unreadable). A slow reader
  drops frames; it never delays the game.
- Bounds: 32 worlds, 16 drivers and 128 traffic cars and 128 rivals per
  world, coordinates within 100 km of the origin, ids at most 2^53 - 1, and
  1 MiB per server line. The supervisor checks every frame against them and
  drops one that fails.

How it travels: the server writes each frame as one JSON line on its
standard output after the readiness line, from its own thread, so a stalled
pipe never blocks a world session. The supervisor's pump keeps only the
newest complete line in memory and writes none of them to
`server.stdout.jsonl`, which keeps just the readiness line. Without
`--events` the server is not asked for frames.

Not established yet: the positions are what the server holds, but no live
run has compared markers with the game's own map, and the map orientation
was checked only by drawing the road network over it.

### Session API

With `--api`, `setup`, `serve` and `start` serve their session over HTTP on
this computer, so a desktop front end can follow and control it the way it
would a server on another computer, and can close, crash or reopen without
affecting the session. The supervisor listens on `127.0.0.1` at a port the
system picks, from just after it locks the data directory until the
session's last status, and answers:

| Request | Answer |
| --- | --- |
| `GET /v1/info` | the `version` record |
| `GET /v1/status` | the `status` record, as `status --json` |
| `GET /v1/events` | the session's `event` records as server-sent events |
| `POST /v1/stop` | the `stop` record: close the game, then the server, as `stop --json` |
| `POST /v1/play` | the `play` record: start the game in a `serve` session, as `play --json` |
| `GET /v1/settings` | the `settings` record: the saved traffic and rivals, as `settings --json` |
| `PUT /v1/settings` | the `settings` record after saving new amounts, as `settings --json` with `--traffic` and `--rivals` |
| `GET /v1/profile` | the `profile` record of the selected save, as `profile --json` |

Answers are the records the matching commands print, with the same schemas
and the same checks behind them. A refused request answers an `api-error`
record (`{"version":1,"message":"..."}`) with status 400 (a malformed
`Content-Length` or settings body), 401 (no or wrong token), 403 (wrong
`Host`, or an `Origin` header), 404, 405 (with `Allow`), 409 (the command
refused, as its exit codes 1 and 4), 411 (a settings body without
`Content-Length`), 413 (a body on any other request, a settings body over
1 KiB or a head over 8 KiB), 415 (a settings body that is not
`application/json`) or 503 (more than 16 connections or 8 event streams).
Every request needs `Host: 127.0.0.1:<port>` (or `localhost:<port>`) and
`Authorization: Bearer <token>`: web pages cannot reach it, and other
programs need the token.

`PUT /v1/settings` is the only request with a body: a JSON object with
`Content-Type: application/json` and a `Content-Length` of at most 1 KiB,
holding the `settings` record's `traffic_percent` (0 to
`max_traffic_percent`), `rival_limit` (0 to `max_rivals`, or `null` for the
original number) or both, such as `{"traffic_percent":200,"rival_limit":null}`.
A field left out keeps its saved value; an empty object, any other field, a
`null` traffic or a value out of range is refused with 400 and saves
nothing. Saving works as the `settings` command does while a session runs:
the file is replaced at once and the running server applies it within about
a second, so the answer has `saved: true` and the new values. The API saves
one request at a time. `GET /v1/profile` reads the save that `account.json`
selects, read-only, with the car names of the installation the session
started; with no save selected it answers 409, as `profile` exits with 4.
Paths in these records (`file`, a save's `path`, car images) are on the
computer that runs the session.

How to reach it: the supervisor writes the `api` record (`version`, session
`id`, `url` such as `http://127.0.0.1:53111`, and a `token` of 64 hexadecimal
characters from the system's random source, new for every session) to
`api.json` in the session directory, readable only by the user where the
system allows, and removes it when the session ends. `nfs-afterhours api
--json` prints it for the current session, or exits with 4 when no live
session serves the API, also while a session that just became current is
still starting it.

The event stream (`text/event-stream`) sends each event as `id` (its place
in the session), `event` (its kind) and `data` (the record's JSON on one
line). It starts with the newest `status` and `map-feed` events, so a reader
that connects or reconnects knows the state at once, then sends every later
`status`, `diagnostic` and `map-feed` event in order, with a `: keep-alive`
comment every 15 seconds while idle, and ends after the session's last
status. `session` and `final` belong to the command's standard output and
are not in it. The supervisor keeps the last 512 events (at most 4 MiB) for
readers that fall behind; a slower reader skips ahead and never holds up
the session. The session's log files keep every diagnostic.

`--api` is for front ends: it silences console text as `--events` does, and
standard output holds only the front end's `session` and `final` event
lines, so a client that starts the command learns why it was refused before
a session existed, and nothing in the session depends on that pipe. With
`--events` as well, every event also goes to standard output. The server is
asked for map frames whenever either is on. The API only listens on this
computer; serving another computer is not offered yet.

### Playing in a serve session

On Windows a `serve` session is ready to host the game unless it was started
with `--server-only`: its preflight runs the same game checks as `start`
(activation helper, client registration, no game or EA app running, port 3216
free) and its server starts as for `start`, under the compatibility name and
with the launcher-services port. Without the game checks passing, `serve`
refuses, exactly as `start` does; `--server-only` serves anyway, without game
support, as on Linux and macOS.

`play` writes a `play` request into the current session directory, as `stop`
does, after checking that the current session is a live `serve` session with
game support, not stopping, with no game starting, running or already
requested, and that no game or EA app process is running. Otherwise it exits
with 4 (no live session) or 1 (anything else) and writes nothing. The serving
supervisor consumes the request and runs the game steps of `start` in the same
session and job: `starting`, `activation` if the EA helper shows a window,
routing, then `ready` with `game_running` true. Statuses and events flow as
for `start`; each game gets its own logs (`game`, then `game-2`, ...).

When that game exits, or fails to start, the supervisor closes it if needed,
finishes its routing lease, drains its logs and returns to `ready` with
`game_running` false and a message with any exit code or reason; the server
keeps running and `play` may start another game. `stop` still closes the game
first, then the server, also while the game is starting. A failure to close
the game or finish its lease, or a server exit, ends the session through the
normal cleanup.

## Sessions

`sessions` lists `artifacts/launcher/<id>/` newest first (by when the session
was requested), 20 by default and at most 200. For each session it reports the
action from `request.json`, `status.json`'s latest state and message (a live
state without its supervisor reads as `interrupted`, as in `status`), start
and end times, every ordinary file with its size, line count and whether it
reached the 4 MiB log bound, the levels in `server.stderr.log`, its `unsupported
operation received` warnings grouped by service and route (a Blaze route's
per-request `correlation` is dropped so repeats group together), whether
`artifacts/runs/<id>/` exists, and on Windows the game's
`Documents\Need For Speed\Temp\CrashDump_*.dmp` files written from the request
until two minutes after the last status. It also reports whether the EA
Desktop log folder exists.

It writes and locks nothing. Every read is bounded: 4096 session folders,
256 entries and 64 listed files per session, 8 MiB per scanned file, 256 MiB
of scanning per call, 16 KiB per parsed log line, 256 warning groups, 4096
crash dump folder entries and 32 dumps per session. A damaged or oversized
record becomes a warning on its session; the listing continues.

## What it does

A session is one run of `setup`, `serve` or `start`. The front-end command
runs preflight, creates `artifacts/launcher/<session-id>/` in the data
directory, writes `request.json` there, removes sessions beyond the newest 20
with their `artifacts/runs/<id>/` recordings (`retention`; the new and the
current session are always kept) and starts a second copy of itself
with the hidden `supervise` subcommand. The supervisor holds the
data-directory lock, records the session's first status (`preparing`,
"Starting the session.") and only then makes it current in
`current-session.json`, so `status`, `stop` and `play` read a starting session
as `preparing`, never as a missing file. A pointer to a session without a
status, which an earlier launcher left while starting, is exit 4 for each of
them. The supervisor then starts the session API if requested, on Windows
creates its own job object with kill-on-close, and runs the session state
machine:

```text
preparing  -> nfs-server prepare-local, then create-account on first use
starting   -> server ready; game started with the date marker       (start, or play in serve)
activation -> the installed activation helper shows a window; no timeout
ready      -> routing applied (start, play) or server listening (serve)
stopping   -> a stop request arrived; close the game, then the server
finished | failed
```

In `serve`, a play request moves `ready` through `starting` (and
`activation`) back to `ready` with the game, and its exit back to `ready`
serving.

Cleanup always runs after the session body, in a fixed order: close the game
if it is still running, retire the routing lease once the game's lifetime has
ended, stop the server through its stop file, drain both children's log
pumps, disarm the job. A cleanup failure is reported together with any
earlier failure, and the job stays armed so process exit still kills the tree.

## Profiles

`profile` reads one account save, `artifacts/account/<id>.sqlite`, for a
desktop front end; `--list` reads every save there. It never takes the
data-directory lock and never writes, so it runs while the server holds the
save. The save is opened through `nfs-storage`'s read-only inspection: a
read-only SQLite connection with `query_only`, no checkpoint on close, at
most a two-second lock wait, and every domain read in one read transaction,
so a profile is always one committed state. Like any WAL reader, SQLite may
leave empty `-wal` and `-shm` files beside a save it found without them.

Values are decoded by the server's own code in `nfs-services`: identity,
bank balance, REP scores evaluated against the installed thresholds, the
single-row tables (`RepValuesTable`, `PrestigeMedalTable`,
`GameplayStatsTable`, `SpeedListStatsTable`, `ProgressionStats`), the five
garage slots (slot 0 is the primary vehicle), every owned car in the garage
and the warehouse, item, livery and table-row counts, and save metadata.
Content comes from `--content-dir`, else the cache the current session
prepared, else the only cache present. Without content the save still reads;
REP, tables, cars and car names are then `null`.

The JSON contract, version 1:

- Every table cell is `{"value": <i32>, "stored": <bool>}`. `stored: false`
  means the save has no cell and `value` is the schema default the server
  serves. Values are the raw stored integers: the game's units for
  distances, times and speeds are not confirmed, so none is applied.
- Only values whose meaning our server defines are formatted: `bank.formatted`
  and `save.updated_at_utc`, next to the raw `bank.balance` and
  `save.updated_at_ms`.
- Item IDs and the persona ID are decimal strings, so JavaScript keeps every
  digit. Definition GUIDs are 32 lowercase hexadecimal characters.
- A car's `name` is the game's own localized name and `names` its long,
  medium and short forms (`name` is `names.long`), read from the player's
  installation each time, never cached or shipped. They are `null` when a
  string does not resolve, with a warning naming the string ID. The
  blueprint stays separate: `vehicle.label` is the blueprint asset name
  without `Car_` and with spaces for underscores.
- The installation is `--game-dir`, else the one the current session
  started, else the game's registered one; the locale is `--language`, else
  the game's registered locale, else `en_US`. `localization` reports both
  and where they came from, or is `null` with a warning when no names could
  be read. Locales whose text database uses a multi-byte encoding (Japanese
  and Traditional Chinese) are not decoded: their names stay `null` rather
  than guessed. Reading names takes about a quarter of a second in a
  release build.
- `cars` lists every owned car, as the server defines one for purchases,
  sales and the warehouse (docs/CAR-OWNERSHIP.md): a root item of the
  race-vehicle class in the owned state. Order: the active car (slot 0),
  garage slots 1-4, then the warehouse (owned cars outside the five slots)
  by item ID. Each has its `location` and `slot`, `name`, `names` and
  `vehicle` (as above), `installed_parts` (records reachable through
  installed links, nested parts and an installed library wrap included),
  `spare_parts` (records the car owns that are not installed: replaced stock
  parts and purchased spares), `item_records` (its ownership tree, car
  included: what a sale removes) and the stored `sale_price` (what a sale
  credits). `car_summary` adds garage slots used of 5, warehouse cars used,
  the server's warehouse limit (100) and the total sale value. Both are
  `null` without item content. Custom wrap and tuning are not reported:
  those edits change stored records in place and the save keeps no stock
  reference that tells them apart without guessing.
- Each car and garage item has `image {medium, large}`, each `{path,
  width, height, source_format}` or `null`, and `car_images` is the image
  directory. Both are `null` until `car-images` has run for the current
  content (see below); a damaged image set is `null` with a warning.
- A domain that is absent or cannot be decoded is `null` and adds a
  sentence to `warnings`. A save that cannot be opened at all is an error
  (exit 1, 4 or 5); in `--list` it is an entry with `error` set.

## Car images

`car-images` decodes each car's two in-game pictures from the installation
(read-only; `--game-dir`, else the current session's, else the registered
one) into the data directory, and `profile` reports the files. No image is
shipped, committed or embedded. A complete set is reused without reading the
installation; a damaged one is rebuilt.

How a name becomes a picture in the supported build, established from the
installed data (every named texture of every car matched; the client code
that does the lookup was not traced):

- A car item's `MediumTextureName` and `LargeTextureName` are not asset
  paths. They are `Id`s of `Output` entries in two `UITextureMappingAsset`s
  of the frontend superbundle, `ui/textures/items/vehiclemediumtexturemappingasset`
  and `ui/textures/items/itemslargetexturemappingasset`.
- An entry imports a `TextureAsset` (all under `ui/textures/`) with a UV
  rectangle; only entries covering the whole texture (0,0 to 1,1) are used,
  which is every car entry today.
- The texture's `Resource` is a 128-byte header in the same bundle's `res`
  list: format (`RenderFormat`), size, mip layout and the pixel chunk's
  GUID. The chunk is listed by SHA-1 in a `Win32/chunks*` table of contents.
- Mip 0 is decoded with `bcdec_rs` (BC1, BC1A, BC2, BC3, BC7) or copied
  (RGBA8, BGRA8). BC1 without alpha is written opaque. Other formats,
  cube maps, arrays and streamed-out first mips are reported as unresolved.

On the supported installation 64 of 75 cars name a medium image and 62 a
large one; all of them decode: medium BC1 256x256, large BC1 512x512 (one
BC3). The large pictures are stored square; how the game crops or stretches
them on screen is not established.

Files go to `artifacts/content/car-images/<content fingerprint>/` as
`<chunk SHA-1>.png` plus `index.json`, beside the prepared content entries
and never inside one, because the content cache refuses to replace an entry
holding files it did not write. A set is written to a temporary directory
and published by one rename. Bounds: 1,024 files and 512 MiB per set,
4,096 x 4,096 pixels and the reader's asset limit per texture, a 1 MiB
index. The `car-images --json` record lists counts per size, formats and
every named image that could not be decoded with the reason.

## World map

`map-images` decodes the game's pause-menu world map from the installation
(read-only; `--game-dir`, else the current session's, else the registered
one) into the data directory for a map view, and reports the world box it
covers. No image is shipped, committed or embedded. A complete set is reused
without reading the installation; a damaged one is rebuilt.

What the supported build holds, established from the installed data:

- The map is the `UITiledTextureAsset` `ui/menu/worldmaptiledtexture` in
  `Win32/levels/genesis01/genesis01`: 256-pixel tiles of a 16,384-pixel map
  at detail levels 1 to 3 (8,192, 4,096 and 2,048 pixels across; level 0 is
  not shipped). Each `Tiles` entry gives its level, column
  (`HorizontalIndex`), row (`VerticalIndex`) and `NormalizedRect`, and imports
  a `TextureAsset` under `ui/menu/tiledtexture/`, decoded as for car images.
  A tile whose rectangle does not match its column and row is not used.
- The world box is the largest unrotated `OBBData` of the UI map's world part
  `levels/genesis01/ui/layer1_uimap`: 8,192 m either side of the origin in x
  and z. A world position maps to u = (x - min_x) / (max_x - min_x) and
  v = (z - min_z) / (max_z - min_z) from the top-left corner, so +z points
  down the image; at full size one pixel is one metre. That orientation was
  checked by drawing the game's road network over the decoded map, not yet
  against a live position. Without a usable box the tiles are still
  written, `world` is `null` and a warning says why.

On the supported installation all 1,344 tiles decode (BC3), about 28 MiB of
PNG in about 9 seconds. Files go to
`artifacts/content/map-images/<content fingerprint>/` as
`<level>-<column>-<row>.png` (columns from the left, rows from the top,
from 0) plus `index.json`, published by one rename like the car images.
Bounds: 4,096 tiles, 8 detail levels, 128 tiles a side, 1,024-pixel tiles,
256 MiB per set and a 1 MiB index. The `map-images --json` record lists the
levels with their tile counts, the file pattern, the world box and every
listed tile that could not be decoded with the reason.

## Why it is built this way

**Two processes.** The supervisor runs separately from the command you typed
so your terminal is never placed in the session job, and so a supervisor
crash is handled by Windows closing the whole tree rather than by code that
just crashed.

**Inherited Windows jobs.** Windows may place even a desktop-launched process
in a job. The supervisor creates a new, empty job nested under any inherited
job, retaining the parent's restrictions. Its cleanup owns only the supervisor
and its descendants, not ancestor or sibling processes. Incompatible job limits
produce a Windows error before children start. Windows subprocess regressions
exercise normal cleanup and a forcibly terminated supervisor, verifying that
session children exit while an unrelated sibling remains alive. These tests do
not launch the game or validate activation.

**Decisions are portable, actions are not.** The state machine in
`supervisor.rs` only decides what to do next. It calls a `Steps` trait, which
the live session implements with real processes and the unit tests implement
with a recording fake. That is how ordering, cancellation at every wait and
cleanup after every failure are tested on any machine without a game.

**One crate may use `unsafe`.** `nfs-afterhours-sys` wraps the Win32 and
libc calls behind safe functions with bounded buffers, and Clippy requires a
`SAFETY` comment on every block. This crate contains no `unsafe` at all.

**Windows first, not Windows only.** `serve` works on Linux and macOS today.
Only game launch, the routing lease that writes 13 bytes into the game
process, the activation-helper window check and the registry read are
Windows-specific, and each sits behind `cfg(windows)` in `platform/`. There
is deliberately no kernel-enforced child cleanup on Unix: `serve` has no game
and no memory lease, so a supervisor crash leaving the server running is
ordinary daemon behavior, and `status` reports it as `interrupted`.

**Fail closed.** The installation must match the supported build by digest.
The routing lease pins the game by handle, creation time, image path and
digest, verifies three code regions by hash, checks the server still owns the
loopback listener, and writes with read-back verification and rollback that
never overwrites bytes it did not write. Any mismatch stops the session and
says so.

**Nothing is hidden in the data directory.** Every file the launcher writes
is small, versioned JSON (`account.json`, `current-session.json`,
`request.json`, `status.json`), a bounded log, or a digest-named copy of our
own server. The formats are the ones the PowerShell scripts wrote, so
existing data directories keep working.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | refused by a safety or precondition check; nothing was changed |
| 2 | invalid command line |
| 3 | a session started and then failed; inspect its session directory |
| 4 | no live session or record to act on |
| 5 | input/output or internal error |

## Layout

| Module | Owns |
| --- | --- |
| `cli` | clap command tree and help text |
| `commands/` | one function per subcommand; `launch` holds the front-end and `supervise` |
| `preflight` | the read-only checks before anything is written or started |
| `supervisor` | the state machine; `supervisor/live` implements it over processes |
| `platform/` | the `Platform` trait, the shared child with bounded log pumps, and the Windows and Unix implementations; `platform/windows/lease` is the routing lease |
| `endpoint` | the 13-byte transaction over abstract memory, tested with injected faults |
| `profile` | read-only profiles from account saves; `profile/content` picks and loads the prepared content; `profile/names` and `profile/images` read car names and pictures from the installation |
| `map` | the world map's tiles and world box, decoded from the installation by `map-images` |
| `contract` | record names, versions, capabilities and schema generation; the strict test validator |
| `sessions` | read-only session folder summaries and the server log scanner |
| `events` | the JSON-lines event stream |
| `api` | the session API: `api/hub` keeps the events for its streams, `api/http` the bounded HTTP subset it speaks |
| `map_feed` | the server's map frames: the newest-line latch, their checks and which become `map-feed` events |
| `settings` | the saved traffic and rival amounts in `settings.json` and their server options |
| `paths`, `records`, `session`, `install`, `digest`, `compat`, `marker`, `clock` | portable pieces with their own unit tests |

## Testing

```sh
cargo test -p nfs-afterhours -p nfs-afterhours-sys
```

Unit tests need no game, server or network; the session API's tests open
loopback sockets on ports the system picks. The integration tests in
`tests/cli.rs` drive the built binary; on Unix they run a complete `serve`
session against a shell script standing in for the server, observe it from a
second process through `status`, stop it, read the same session as an event
stream, follow and stop another through the session API (`--api`), reading
and changing its settings and reading its profile there, and also
kill a supervisor to see `interrupted` reported. On every
platform they check `version`, `schema`, `sessions` and a refused event
stream. The Windows process-layer tests run in CI on a
Windows runner. Live acceptance on the supported installation is recorded in
`docs/LOCAL-LAUNCHER.md`, not claimed by tests.
