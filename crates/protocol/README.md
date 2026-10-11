# nfs-protocol

Typed backend payloads, XML2 redirector and latency QoS codecs, and bounded
bit-oriented world messages. Dependencies are `nfs-heat2` and `quick-xml`;
Fire2 is used only by framing integration tests.

Optional and unknown fields retain their wire meaning. Unsupported alternatives
return explicit errors. A schema does not imply a successful route, account
authorization, matchmaking policy or world readiness. Inferred associations and
unverified variants remain identified in API documentation.

Tests here use independently constructed inputs. Private capture comparisons
belong to external reference tooling, which consumes these same libraries.

Daily challenges support typed response construction, including the nested award
map/list layout. The retained-byte view remains available for inspection. Neither
codec selects a catalog, rotates challenges or grants account progress/rewards.
