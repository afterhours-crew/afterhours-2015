# nfs-services

State-owned services built on the storage port and typed protocol models.

The utility text check (`9/20`) uses an explicit local policy for garage text:
UTF-8 without control characters, up to 512 bytes, passes unchanged. Longer text
receives the native string-too-long result; invalid text receives the other-error
result. Each response retains the request's entry order and correlation. The
service is stateless, does not contact EA and does not implement EA's moderation
rules. Local ingress limits are 32 entries, 1,024 bytes per input and a 40 KiB body;
malformed or oversized requests receive no successful reply. Text is not logged.
The plate editor's seven-byte limit is enforced separately by its item save.

Static control catalogs (`7/15`, `2050/75`, `2050/78`, `2055/1`) consume a
bounded `nfs-control-catalogs` version-1 document with named key scopes, ordered
integer pairs, kill-switch names and SpeedList type definitions. Limited features
use an explicit disabled policy. Replies are built through typed codecs and bind
the authenticated persona where required. Unknown forms receive no answer; the
edge must not fall back to templates on these routes. This does not implement
feature scheduling, SpeedList matchmaking or account history.

Item inventory assurance (`2052/19`) acknowledges a recognized declaration
without awarding items or modifying account grants. `nfs-item-licenses` version 1
accepts its exact list or an empty declaration; version 2 accepts any
duplicate-free subset of configured license/source pairs in any order. Unknown
names, sources and item systems remain unsupported. The version-2 vocabulary
can be generated from the player's installation and is not an ownership claim.

Daily challenge reads join an explicit local catalog/period with current durable
account progress, obtained awards and monthly ranks. Empty local history grants
nothing. Reads preserve state, and validated state operations can share atomic
progression batches. Period/time values remain opaque configuration; automatic
rotation and gameplay reward policies are not inferred by this service.

- Client-state acknowledgement validates a complete canonical request and returns
  an empty reply with its correlation. Only modes 1/3 with normal status are
  accepted; this reports no menu-readiness or persistent-state transition.
- Telemetry (`4/171`) validates canonical requests, pins the first accepted
  connection identities and retains at most 16 diagnostic samples. It returns a
  correlated header-only acknowledgement without forwarding data or changing
  account state. Requests contain at most eight reports and 512 body bytes.
- In-game recommendations (`2050/21`) answer only for the current persona,
  encoding empty speed-wall maps for stat types 0 and 1. Shared-wrap listings
  (`2052/23`) likewise require the current persona and supported item system,
  returning a zero total for the empty local catalog. Populated catalogs and
  wrap user-list queries are explicitly unsupported.
- Speed-wall brief reads (`2050/79`) return the account's saved row, or the
  defined Autolog not-found error when a supported own-persona query has no row.
  Empty accounts gain no rank or score. Completing the request allows the
  client's shared content queue to continue to wrap-header downloads.
- Inventory loading initializes an empty account once, validates item graphs,
  preserves later progress and produces replies only from committed state.
- ItemBuilder begin-update validates a complete bounded call against the current
  scene binding and an owned Player (distinct from its Participant). Its server behavior is a silent no-op;
  it neither changes inventory nor accepts commit or customization mutations.
- Persistent tables initialize missing schemas transactionally, preserve sparse
  saved cells and expose garage slots as a projection of the same account state.
- Stats definitions (`7/4`) use a bounded `nfs-stat-definitions` version1 catalog.
  Its groups name a persistent table and ordered integer columns. Current Stats
  queries (`7/16`, followed by `7/50`) read those columns from a committed account
  view, bind the authenticated persona and echo the requested view ID. Missing
  state, other identities, unknown groups and unsupported periods get no success.
  Static default strings never substitute for missing account values.
- Reputation projects level and adjacent score thresholds from the current
  six persisted scores and an injected increasing threshold list. Current Stats
  use that calculated level even when the saved level is stale. The same model
  can drive world presentation; score accrual is outside this component.
- Menu Stats and Awards (`2050/71`) project the same account's collection and
  objective flags, gameplay counters, medals, SpeedList counts and reputation.
  Version1 account metadata stores record names/screenshots/timestamps, race
  records, social counters and entitlement declarations in six bounded local
  tables. Explicit initialization creates empty local history without grants;
  partial or malformed history fails. Metadata operations can share one atomic
  repository batch with related progression changes. The response requires the
  authenticated persona and a single committed generation. Gameplay reward and
  social mutation policies are outside this read service.
- Startup groups own a single authenticated member, fresh injected identifiers,
  and create/mesh/finalize/initialize transitions. Public and private group
  requests have explicit validation; setup fields come from current identity,
  network data and a bounded `nfs-group-policy` version1 deployment document.
  No captured request or response is a configuration input. Immediate retries
  repeat only the correlated reply; membership notifications occur once.
  The edge must commit each output batch after writing it or abort on failure.
  An initialized, committed group exposes an immutable matchmaking handoff.
  Multiplayer membership, matchmaking and driving worlds are outside this service.
- Versioned item-definition and table-schema loaders validate bounded input and
  its build identity. Callers supply their own content; no catalog is bundled.

Bootstrap pre-authentication, ping and IdentityParams use a versioned deployment
document and caller-bound endpoints on the server's listen address (loopback,
or one concrete network IPv4 address). Configuration includes product
metadata, bounded scalar settings and latency site names; endpoint values are
generated locally, and the bandwidth site stays unset. Each connection commits
its ordering transition only after a complete write. Exact retries preserve time;
failed writes close the exchange. This does not implement account authentication.

Time and local account identity are explicit inputs. Repository/file operations
are synchronous; a socket edge must dispatch them to blocking workers. These
libraries provide no listener and require no service access or recording.
The caller creates separate service state for each connection and supplies its
authenticated local persona. These handlers accept one complete frame; stream
assembly and session-readiness gating belong to the calling edge. Invalid input
produces an error without advancing service state.
The local initial garage policy selects the lowest owned root vehicle ID;
it is an explicit local policy, not verified official fresh-account selection.

`garage` manages five persistent slots. A purchase atomically reserves an empty
secondary slot, debits the installed price and creates a distinct car with its
stock parts. Shared account wraps are granted only when missing. Selection swaps
an identified secondary car with the primary. Resale requires a secondary car,
credits its stored sale price, removes its installed and spare descendants, and
retains account-owned wraps. Select another car before selling the primary.

Car transactions use server-issued account session epochs and bounded persistent
sequence receipts. A repeated successful command in the same epoch returns the
original result; superseded or evicted commands cannot charge again. A new
connection receives a fresh epoch, so its restarted sequence space remains
independent. The socket edge serializes these writes with equipment and REP,
waits for purchase-result delivery before displaying new item references, and
replaces only garage slots whose car identities changed.

Tests construct item definitions, rows and accounts, including separate-account
initialization races, failed commits, retries and SQLite restart persistence.
Control-service tests cover malformed and noncanonical requests, resource bounds,
identity changes, connection isolation, repeated requests and correlated replies.
Run the workspace Cargo gates in the root README. No game install is required.

Local authentication uses an explicit persistent identity (storage-account
binding, positive wire persona/account IDs and display name), deployment policy,
and fresh caller-supplied entropy per connection. The edge authorizes the local
account and compares its storage binding before serving. External auth codes are
shape-checked only and never retained, verified remotely or returned. Login emits
four ordered frames; post-auth advertises the auxiliary listener's address for
telemetry and the ticker. State advances only
after the entire batch writes; partial failure closes the exchange. Retries with
the same correlation retain the first clock value. Persistent identities survive
reconnect while session/telemetry/ticker tokens and connection IDs refresh.
Remote account authentication and automatic first-login history are not provided.

`fresh_account::Prepared` takes injected entropy/time, a chosen name, explicit
local license policy, an Items initialization recipe and typed table defaults.
It constructs fresh state without an existing profile, then publishes service
domains and inventory/table state in one transaction in a new directory. It does
not implement the official prologue award protocol or infer missing menu replies.

`progression` owns the installed objective catalog and activation expressions;
the content builder reexports these types. `story::Policy` uses that same catalog
to inspect saved objectives and prepare atomic checkpoint/completion batches.
`event_rewards::commit` can join an attributed story objective to the same
transaction as race cash, REP, earned-cash statistics, records and unlocks. It
reads the result's retained retry receipt before rebuilding effects; a later
story operation or restart does not make an old result payable again. Missing
receipts retain the active-objective checks. The caller must establish the owned
result and its authored completion conditions; this service does not infer a
mission from a payout or require a win. The server's selected result contracts
carry their attributed objective into this transaction. Portable service and
worker checks do not establish full mission or live-game acceptance.

Intro ordering must have a single next objective at each step; main-story
completion preserves independent active objectives and projects their authored
prerequisites. Checkpoint editing remains limited to the intro. Existing main-story
completions and unrelated items/tables are retained. A versioned `AfterhoursStory`
table records played, edited and inherited completion, plus the last monotonic
operation receipt. Reward operations supplied by the owning domain commit with
completion, and claimed rewards cannot be granted again after a rewind. Partial
objectives, malformed receipts, stale generations and reused intents are refused.
Version2 covers both branches. Version1 intro-only ledgers remain readable and
migrate atomically on the next mutation; earlier main completions retain existing
provenance and cannot acquire newly earned rewards. Reading or preparing a plan
does not mutate the save. `Policy::objective` exposes one objective's flags and
completion provenance without assigning a linear main-story position.
The caller must establish objective success and serialize commits; this module
does not play the intro or validate arbitrary client-reported race results.

Host readiness owns the current binding and one bounded pending mesh request.
It requires matching reliable-synchronization proof, a later injected join time,
and a complete ACK/player-ready/player-joined batch write before emitting a
move-only continuation permit. Notifications are typed projections of this state;
no captured bodies are retained. Foreign/dropped writer tickets, partial writes,
wrong proof identities and malformed requests fail closed. Retries emit only the
correlated ACK after readiness commits. The edge remains responsible for admitting
the current world after its setup/self-validation writes and for publishing real
transport synchronization; construction does not prove those prerequisites.
