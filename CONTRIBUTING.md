# Development and component promotion

This repository owns production implementation, runtime dependencies, portable
configuration, operator documentation and self-contained tests. It grows one
settled, tested component at a time. A working end-to-end server is not required
before a component can be promoted.

For each promotion:

1. Select a coherent implementation and all first-party dependencies it needs.
   Review every file and its provenance. Import selected files into a normal
   feature branch; do not import another repository's history.
2. Keep raw captures, disassembly, binary dumps, extracted game assets, personal
   profiles, databases, credentials and private research notes out of this
   repository. `.gitignore` is a convenience, not a content review.
3. Bring meaningful, self-contained tests using constructed inputs. Reference
   capture comparisons can run in the separate research workspace. Record their
   results there; do not ship their private inputs here.
4. Build and test from this checkout alone. No private Git dependency, sibling
   checkout, hidden fixture path or research tool may be required. Preserve
   database and protocol behavior during moves; document any deliberate change.
5. Open a pull request with scope, validation and limitations. The owner merges.
   External research consumers pin a reviewed commit from this repository and
   retire their duplicate implementation. Future runtime fixes happen here.

Dependency flow is one-way: external development/reference tools may consume
these libraries; these libraries must never consume the private research tree.
Settled code can be promoted while dependent components are still under research.
Do not claim a component is game-validated based only on unit or reference tests.

## Architecture and gates

Keep protocol codecs, transport, owned domain state and persistence separate.
Protocol cores take input plus injected time/randomness and return output; socket
and async adapters remain at the edge. Bound input sizes and retained state.
Use per-account/session ownership, transactional progression and versioned
migrations. Runtime replies must be generated from owned state, never replayed
from recordings. Local operation must not require public authentication,
telemetry or hosted storage.

Commit `Cargo.lock`; change dependency versions deliberately. First-party Rust
forbids unsafe code. Every code change must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Review third-party licenses before adding code or dependencies. Current storage
uses pinned `rusqlite` with bundled SQLite and limits; transitive versions and
registry checksums are recorded in `Cargo.lock`. XML2 codecs use `quick-xml`
0.42.0 (MIT); its `memchr` dependency is MIT OR Unlicense. These come from the
Cargo registry; implementations are not vendored. The LSX AES arithmetic is
original code tested with NIST standard vectors.

The installation reader uses lz4_flex 0.14.0 (MIT, safe decoding only) and
miniz_oxide 0.9.1 (MIT OR Zlib OR Apache-2.0; its adler2 dependency is 0BSD OR
MIT OR Apache-2.0). Content fingerprints use sha2 0.11.0 and its RustCrypto
dependencies (digest, block-buffer, crypto-common, hybrid-array, cpufeatures,
typenum, cfg-if, libc), all MIT OR Apache-2.0.
Content loaders use serde_json 1.0.151 (MIT OR Apache-2.0); all registry
versions and checksums are locked. Main/master/default require pull requests
and reject force pushes/deletion, including for administrators.

Loopback discovery and QoS support uses httparse 1.10.1 (MIT OR Apache-2.0)
and openssl 0.10.81 (Apache-2.0). Its vendored build uses openssl-src
300.6.1+3.6.3 (MIT OR Apache-2.0 wrapper; OpenSSL 3.6.3 is Apache-2.0).
These dependencies are fetched from the Cargo registry, not copied here.

The server edge uses Tokio 1.53.2 (MIT), tracing 0.1.44 (MIT) and
tracing-subscriber 0.3.23 (MIT). Versions/checksums are locked; these dependencies
remain at the socket/diagnostic edge rather than in the sans-IO service models.

## License

All code here is under the [Mozilla Public License 2.0](LICENSE). Contributions
are accepted under the same license, with no separate contributor agreement.
Start every new source file with the three-line MPL notice used throughout the
tree, and keep `license.workspace = true` in each crate manifest.
