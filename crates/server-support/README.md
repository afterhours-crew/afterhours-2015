# Local socket support

`nfs-server-support` owns bounded loopback discovery and latency services.
Discovery parses a strict HTTP/XML request and advertises the caller's already
bound local listener. QoS uses a fresh process-local certificate authority, at
most four TLS connections and 128 UDP datagrams under one absolute deadline.
It does not change the operating-system trust store or contact public services.

These adapters perform blocking I/O. Run them on a blocking worker; protocol
and account services remain separate. Raw observations can contain identifiers
or credentials and deliberately have no `Debug` implementation. Persist them
only through an explicitly selected private diagnostic sink.

Tests construct HTTP/XML requests and use temporary loopback endpoints, bounded
deadlines and generated keys. No game files, captures or private repository are
needed. This library does not establish game or offline-play support.
