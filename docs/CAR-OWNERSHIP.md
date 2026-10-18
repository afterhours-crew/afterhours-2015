# Car ownership and garage management

Implementation and game acceptance are tracked in [issue #52][tracking]. The
current branch has automated coverage; its combined in-game acceptance is pending.

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
result delivery, placement, switching, a car-specific edit, sale and slot clearing.
These checks do not establish game acceptance. Warehouse storage and the other
systems on the [gameplay roadmap][roadmap] remain outside this issue's five-slot
scope.

[tracking]: https://github.com/afterhours-crew/afterhours-2015/issues/52
[roadmap]: https://github.com/afterhours-crew/afterhours-2015/issues/62
