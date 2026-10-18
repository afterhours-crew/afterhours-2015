# Car ownership and garage management

Implementation and game acceptance are tracked in [issue #52][tracking]. The
five-slot and warehouse scopes passed combined live checks on 2026-10-18,
including fresh game/server restarts. [Issue #98][warehouse] records warehouse
acceptance; the milestone is ready for owner review in PR #97.

The local account owns five garage slots. Slot zero is the active car; the other
four hold secondary cars. Buying an eligible car reserves a free secondary slot,
deducts its installed purchase price, and constructs a distinct stock inventory
tree in one transaction. Two cars of the same model have separate identities,
parts, paint and tuning. Installed single-quantity offers cannot be bought twice
while the account owns that definition. Insufficient funds and a full garage
reject the purchase without a debit.

Selecting a secondary car swaps it with the active slot. Requests without an
explicit car reference resolve against the active slot. Explicit part and tuning
requests retain their ownership checks. Selection and each car's inventory are
persisted; switching cars discards unsaved appearance previews.
Garage selection and placement commands identify the owned participant object;
the separate player object is rejected on this route.

The garage Sell action accepts one owned secondary car. Select another car before
selling the active car. A sale credits the vehicle's stored sale price and removes
all records owned by it, including installed parts and purchased spare parts.
Those parts do not add a separate refund. Account-owned wraps remain available;
a wrap granted with a car is added only when the account does not already own it.
The sale, freed slot, inventory deletion and cash credit commit together.

Move to Warehouse clears a secondary garage slot while keeping the car and all
its inventory records. Select another car first to store the active car. The
warehouse consists of owned cars outside the five garage slots; it requires no
second inventory or copying of customization. Retrieval binds a stored car into
a secondary slot. Swap From Warehouse replaces that slot's existing car, which
becomes stored in the same transaction. Cash and owned items are unchanged.
The local limit is 100 stored cars, advertised to the client and enforced before
transfer. Swapping remains possible at capacity because the stored count does
not increase. Only the account's owned vehicle roots can be transferred.

The repository and item codec allow at most 8,192 records per account; item Files
retain their 1 MiB byte limit and bounded depth/reference counts. The installed
largest stock tree, repeated across 100 warehouse cars and four secondary slots,
plus the initial car, uses 4,987 records and 394,483 encoded bytes. Extra spares
and customization remain subject to these finite resource limits.

Transactions use bounded persistent receipts keyed by a server-issued connection
epoch and client sequence. A repeated successful request in that epoch returns
its original result. Superseded requests and evicted epochs are rejected. A new
connection has a fresh sequence space; the protocol does not identify a new
connection's purchase as a retry of an earlier connection's request.

The server waits for delivery of the item transaction result before replacing
garage display objects and publishing new item references. Only changed slots
receive new display lifetimes. Delayed acknowledgements from retired displays
cannot acknowledge the new ones. Installed content supplies the GarageEntity
serializer binding, purchasable offers, stock parts and vehicle profiles,
including headlight and spoiler animation fields.

Automated checks cover transaction retries, funds, quantities, the five-slot cap,
duplicate car models, account isolation, concurrent purchases, SQLite restart,
selection, sale descendants and wrap retention. The edge test exercises purchase,
result delivery, placement, switching, a car-specific edit, sale and slot clearing;
every generated section is encoded and applied to a receiver model. In particular,
a deletion-only section has no construction setup.

Live acceptance used five cars, including the Mustang Foxbody, Mazda MX-5,
Toyota Supra, Mercedes-AMG GT and Ferrari 458 Italia. Purchases deducted their
prices and appeared in free slots. Two subsequent sales removed their displayed
cars without disconnecting; credits, descendant removal and account-wrap retention
matched the saved state. Freed slots could be reused, and the full garage offered
no sixth purchase. The owner confirmed separate paint and tuning remained on the
original cars after restart. A final independent restart restored the selected
Ferrari and all five cars, and switching to the green MX-5 succeeded. The final
saved audit retained all 311 owned records, including its installed air filter and
tuning, unchanged; only active/secondary slot selection changed. Both final runs
ended through normal game exit.

The short black transitions remain stock client behavior. The installed garage UI
configures delays of 1.5 seconds for selection, 2 seconds after sale completion and
3 seconds after purchase completion, followed by a 0.5-second closing fade.
Observed backend sale delivery took about 300 ms; selection queued its two changed
displays within milliseconds. The owner reported roughly 3-second purchase/sale
pauses and 1–2-second selection pauses. These are approximate live observations,
not frame-accurate measurements. The timing investigation is [issue #96][timing];
this milestone does not shorten the client's fades or modify game files.

All 54 eligible installed stock-car offers passed private transaction and display
construction comparisons; live testing covered the cars above, not every model.
Retry, malformed-request and concurrent-account guarantees have automated coverage,
not live malformed-traffic tests. Warehouse transfer, retrieval and swaps have
portable and installed-content coverage, including retries, stale slot reuse,
capacity, concurrent retrieval and SQLite restart. The edge test applies generated
removal/retrieval sections to a receiver model while preserving customization.

Warehouse live checks stored the customized MX-5, retrieved it into the freed
slot, stored it again and bought a Volvo 242 into that slot. With all five slots
occupied, swapping the stored MX-5 for the Volvo retained both cars. A fresh
game/server restart restored both stored cars in the warehouse list; collecting
the MX-5 and selecting it as the active car restored its green wrap. All 350
owned records, including installed parts and tuning, remained identical across
storage, retrieval, swapping and restart; only slot bindings changed. The Volvo
remained stored, and the run ended through normal game exit. The 100-car limit
and hostile/concurrent request cases have automated coverage, not live tests at
that scale. Other gameplay systems retain their own roadmap issues.

[tracking]: https://github.com/afterhours-crew/afterhours-2015/issues/52
[timing]: https://github.com/afterhours-crew/afterhours-2015/issues/96
[warehouse]: https://github.com/afterhours-crew/afterhours-2015/issues/98
[roadmap]: https://github.com/afterhours-crew/afterhours-2015/issues/62
