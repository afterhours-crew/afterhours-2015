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
forbids unsafe code, with one exception: `nfs-afterhours-sys`
(`crates/launcher-sys`) wraps the operating-system calls the launcher needs
(job objects, process memory, thread lookup, the TCP ownership table, a
registry read, console events, Unix signals). It is the only crate allowed
`unsafe`; every block carries a `SAFETY` comment, Clippy rejects one without,
and the crate exposes only safe functions over bounded buffers. Keep new OS
calls there, and keep the launcher itself (`crates/launcher`) safe.
Every code change must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The [CI workflow](.github/workflows/ci.yml) runs the same three gates on every
push to a `codex/` branch and every pull request. Tests run on Linux for every
push and additionally on Windows for pull requests and main, alongside the
Windows Rust launcher and process-wrapper tests. Hosted runners are elevated,
so live non-admin session cleanup still needs a local check. A red run must be
fixed before a pull request is reviewed.

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

The launcher uses clap 4.6.7 (MIT OR Apache-2.0) with default features off
(`std`, `derive`, `help`, `usage`, `error-context`); its tree adds clap_builder,
clap_derive, clap_lex, anstyle and heck, all MIT OR Apache-2.0. The
operating-system crate uses windows-sys 0.61.2 and libc 0.2.190, both
MIT OR Apache-2.0 and already locked by other crates. The read-only `profile`
command links the first-party `nfs-storage`, `nfs-services` and
`nfs-world-core` crates, so the launcher also carries the pinned `rusqlite`
with bundled SQLite; no new registry crate is locked for it.

The launcher's JSON Schemas are generated by schemars 1.2.2 and its derive
macro schemars_derive 1.2.2 (MIT). Newly locked dependencies are dyn-clone
1.0.20, ref-cast 1.0.27, ref-cast-impl 1.0.27 and serde_derive_internals 0.30.0,
all MIT OR Apache-2.0; the macros use the already locked syn, quote and
proc-macro2.

Terminal presentation uses anstream 1.0.0 and anstyle 1.0.14 (MIT OR Apache-2.0).
New locked transitive dependencies are anstyle-parse 1.0.0, anstyle-query 1.1.5,
anstyle-wincon 3.0.11, colorchoice 1.0.5, is_terminal_polyfill 1.70.2,
once_cell_polyfill 1.70.2 and utf8parse 0.2.2, all MIT OR Apache-2.0.
Only the terminal output is styled; saved child logs and JSON remain plain.

The Windows bundle embeds its owner-supplied icon at build time with
embed-resource 3.0.12 (MIT). Its newly locked build dependencies are
equivalent, hashbrown, indexmap, rustc_version, semver, serde_spanned, toml,
toml_datetime, toml_parser and toml_writer (MIT OR Apache-2.0), plus vswhom,
vswhom-sys, winnow and winreg (MIT). They are build tools, not runtime services;
the package notices include their license texts.

Loopback discovery and QoS support uses httparse 1.10.1 (MIT OR Apache-2.0)
and openssl 0.10.81 (Apache-2.0). Its vendored build uses openssl-src
300.6.1+3.6.3 (MIT OR Apache-2.0 wrapper; OpenSSL 3.6.3 is Apache-2.0).
These dependencies are fetched from the Cargo registry, not copied here.

The server edge uses Tokio 1.53.2 (MIT), tracing 0.1.44 (MIT) and
tracing-subscriber 0.3.23 (MIT). Versions/checksums are locked; these dependencies
remain at the socket/diagnostic edge rather than in the sans-IO service models.

## Local packet-loss diagnostics

Normal server builds do not include packet-loss controls. For a copied test
account, build with `cargo build -p nfs-server --features network-test --locked`
and run `nfs-afterhours start` with `--record-protocol` and `--server` pointing to
that debug executable. Both the feature and raw recording are needed.
This diagnostic does not need administrator access or change Windows networking.

After reaching the action being tested, create `world-1-loss.txt` in the current
run's **protocol recording directory** (`artifacts/runs/<run-id>` under the chosen
data directory), with exactly `drop-incoming-2000ms`, UTF-8 without a BOM or newline.
Use the connection number in the `world client` log if it is not 1. For example:

```powershell
$command = Join-Path $runDirectory 'world-1-loss.txt'
[IO.File]::WriteAllText(($command + '.tmp'),
    'drop-incoming-2000ms', [Text.UTF8Encoding]::new($false))
Move-Item -LiteralPath ($command + '.tmp') -Destination $command
```

The server checks every 200 ms, logs when armed, then discards incoming datagrams
from that world's established peer before transport processing, for at most two
seconds or 256 packets. Recorded incoming bytes are retained with a diagnostic
loss note. Other connections are unaffected. Each connection consumes only one
command attempt; an invalid file disables that attempt. The file is left in place
as evidence, and a new run always has a new recording directory. Compiling the
feature alone does not drop traffic. Do not use a normal gameplay account for
fault testing, and do not treat a paused process as evidence of packet loss.

Check the feature separately as well as the normal workspace gates:
`cargo clippy -p nfs-server --all-targets --features network-test --locked -- -D warnings`
and `cargo test -p nfs-server --features network-test --locked`.

## License

All code here is under the [Mozilla Public License 2.0](LICENSE). Contributions
are accepted under the same license, with no separate contributor agreement.
Start every new source file with the three-line MPL notice used throughout the
tree, and keep `license.workspace = true` in each crate manifest.

## Tracking work

Gameplay coverage and remaining systems live in GitHub issues. Start from the
pinned [gameplay roadmap][roadmap]: pick a `kind:tracking` issue, work against
its acceptance list, and open linked sub-issues for anything new you discover.
Pull requests reference the issue they advance and state what was live-tested.

[roadmap]: https://github.com/afterhours-crew/afterhours-2015/issues/62
