# nfs-world-core

Socket-free world components: bit writing, transport transforms, datagram
framing and cursors, handshake, reliable link, application admission/control
state, File records and transfer state, and item/inventory models.
Startup content codecs cover load-level messages, names and registrations.
Registration bindings validate distinct name/level/parent/bundle identities and
reject missing parents, cycles and duplicate identities atomically. The included
sequential registration allocator covers the supported startup form only.

Each connection owns its state. Callers supply data, clocks, identifiers, random
values, keys and resource policies. No MAC template, extracted item catalog,
profile, replay queue, scene replication or network listener is bundled. The
application model covers a single-peer host connection. `ClientHandshake` and
`Link::respond` also support a requesting transport endpoint; after validating
application admission, `Application::negotiated` shares the normal fragment,
history and delivery path with it. It starts a fresh connection and rejects
assumed-delivery policy. Callers still own application identity validation and
every decision about accepting handler frames. These APIs do not implement a
game client, mid-session restoration or a complete multiplayer world.

`handshake::Control` exposes the exact request/reply/confirmation body codec
shared by both endpoints. It rejects incomplete or trailing data, including on a
retried request. Decoding a body alone does not validate its transport header,
peer identity, token, cursor or handshake order; endpoint admission retains those
checks. Diagnostic consumers can distinguish these controls from link frames
without carrying a second handshake parser.

Tests cover loss, retries, ordering, rollover, limits, cancellation and constructed
transfers, plus independently packed startup literals and registration allocation.
[The rollover vector](tests/fixtures/transport/README.md) is synthetic.
These checks do not establish offline game acceptance.
