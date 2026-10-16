# Local socket support

`nfs-server-support` owns bounded loopback discovery and latency services.
Discovery parses a strict HTTP/XML request and advertises the caller's already
bound local listener. QoS uses a fresh process-local certificate authority. Its
finite observation helper accepts at most four TLS connections and 128 UDP
datagrams under one absolute deadline. The server uses the bounded TLS request
adapter inside its own persistent Tokio listener; those observation quotas do
not limit server lifetime. A cancellation-aware variant checks an atomic flag
between socket waits of at most 100 ms and retains the same wire/header bounds.
It does not change the operating-system trust store or contact public services.

These adapters perform blocking I/O. Run them on a blocking worker; protocol
and account services remain separate. Raw observations can contain identifiers
or credentials and deliberately have no `Debug` implementation. Persist them
only through an explicitly selected private diagnostic sink.

Tests construct HTTP/XML requests and use temporary loopback endpoints, bounded
deadlines and generated keys. No game files, captures or private repository are
needed. This library does not establish game or offline-play support.
