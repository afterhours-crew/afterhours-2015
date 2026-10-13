# nfs-lsx

See the [local launcher integration guide](../../docs/LOCAL-LAUNCHER.md) for
ownership, request coverage, resource limits, validation and extension points.

Connection-owned, sans-IO local launcher state built on nfs-lsx-codec. The caller supplies a profile and fresh entropy; no default account, sockets, files or external authentication are used.

The state machine performs the challenge exchange and transport switch, then generates configuration, profile, settings, game information, numeric connectivity state, account presence and local authorization-code replies. Unknown requests remain unsupported. GetAuthCode parses ClientId and Scope and returns AuthCode with a value attribute. Codes are fresh per connection and stable for retries within that connection. They are local compatibility values, not EA credentials or a multiplayer authentication boundary. The loopback backend currently shape-checks codes and binds the operator-selected local account.

Frame/XML/transform bounds come from the codec. Tests use constructed data and cover ordering, fragments, malformed input, rejection, routing, account isolation and reconnects. Passing them does not establish cold disconnected play. The server edge owns cancellation, deadlines and entropy generation.

GetPresence reports only the selected account. SetPresence validates that identity and bounded text before updating connection-owned presence; repeated reads return the latest accepted state. Unknown accounts and invalid values cannot mutate it. Presence resets on reconnect and is not durable progression or a multiplayer presence directory.
