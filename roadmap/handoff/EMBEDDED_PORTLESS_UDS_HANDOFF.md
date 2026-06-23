# Handoff: Portless Embedded Mode over Unix-Domain Sockets (UDS-for-all-surfaces)

Date: 2026-06-23
Decision owner: Vijay (singhvjd@gmail.com)
Parent context: `roadmap/handoff/EMBEDDED_CODEGRAPH_BOUNDARY_HANDOFF.md` (the code-graph
boundary work). This handoff is the **portless transport** half that was split out
of that effort.

---

## Decision (locked)

Embedded mode must be **portless**. Today the embedded `proximadb-server`
subprocess binds **three TCP ports** (REST, gRPC, Arrow Flight); Victor's
`start_embedded_db` picks free ones to dodge collisions. That is exactly the
port-picking we want to delete.

**Target shape: bind all three surfaces — REST, gRPC, and Arrow Flight — to Unix-domain
sockets (UDS) under the embedded data dir instead of TCP.** No TCP ports, no
free-port selection, structural per-instance isolation (different data dir →
different socket path → cannot collide).

> Rejected alternatives:
> - *Free-port-per-surface over TCP* — the status quo; the thing we're removing.
> - *True in-process PyO3 (`proximadb_embedded`)* — the repo's eventual north star
>   (zero subprocess, zero ports), but it requires rewiring Victor's
>   `start_embedded_db`/`ProximaRepoConnection` onto the in-process API and aligning
>   its graph API to `ProximaDBGraph`. Out of scope here; revisit as a later arc.
> - *UDS for Flight only* — half-measure; graph would still ride REST over a TCP port.

This aligns with the embedded co-design re-alignment (memory
`project_embedded_codesign_2026_06_22`): "port-free core ← {server = only
port-binder, embed seam}".

---

## The socket-path convention (authoritative — implement exactly this)

1. **Local data dir, path within the UDS limit** → sockets live **under the data
   dir** so they are contained and lifecycle-bound:
   ```
   <data_dir>/sockets/proximadb-embedded.rest.sock
   <data_dir>/sockets/proximadb-embedded.grpc.sock
   <data_dir>/sockets/proximadb-embedded.flight.sock
   ```
   Deleting the data dir removes the sockets with it.

2. **Object-store data dir (`s3://`, `gs://`, `az://`, …) OR a local path too long
   for the UDS cap** → a Unix socket cannot live there (sockets are local-FS only,
   and the path must fit the OS limit), so fall back to a **local tmpdir with a
   collision-proof name**:
   ```
   <$TMPDIR>/proximadb-embedded-<uuid4>/{rest,grpc,flight}.sock
   ```

3. **UUID4 over timestamp** for the unique segment. Timestamps can collide under
   rapid/parallel launches; UUID4 is collision-proof. (A timestamp may be added as
   a human-readable prefix, but UUID4 is the uniqueness guarantee.)

4. **Cleanup:** unlink the sockets on `stop()`/drop; for the tmpdir fallback,
   remove the whole `proximadb-embedded-<uuid4>/` dir.

### Hard caveat: UDS path length
`sockaddr_un.sun_path` caps the socket path at **~104 bytes on macOS, 108 on Linux**.
pytest `tmp_path` lives under `/private/var/folders/<...>/T/<...>` on macOS, which
combined with `/sockets/proximadb-embedded.flight.sock` **can exceed the cap**.
Therefore rule (1) must measure the resolved path length and **fall through to rule
(2)** when `len(full_socket_path) > limit`. Do not assume the data-dir path is short.

---

## Server changes (ProximaDB)

The Arrow Flight server binds a TCP `SocketAddr` only — there is **no UDS support
anywhere in `src/network/` today** (verified: no `UnixListener`/`serve_with_incoming`/
`grpc+unix`). Add an opt-in UDS bind to each surface, selected by config.

- **Config (`[api]`)**: add a transport mode. Suggested:
  `transport = "tcp" | "uds"` (default `tcp` — mixed-read-safe, opt-in per CLAUDE.md
  storage-format/IO migration discipline) plus `socket_dir = "<path>"`. When
  `transport = "uds"`, ignore `rest_port`/`grpc_port`/`arrow_flight_port` and derive
  socket paths from `socket_dir` per the convention above.
- **gRPC + Arrow Flight (both tonic):** swap `Server::builder().serve(addr)` for
  `...serve_with_incoming(UnixListenerStream)` over a `tokio::net::UnixListener`.
  Entry points: `src/network/arrow_ipc/server.rs` (`ArrowFlightServer`, takes a
  `SocketAddr` — generalize to an enum bind target), and the gRPC server in
  `src/network/multi_server.rs`.
- **REST (axum/hyper):** serve over a `UnixListener` (axum supports
  `axum::serve(UnixListener, app)` on recent versions; otherwise hyper
  `Server::builder(UnixAcceptor)`). Entry: `src/network/rest/server.rs`.
- **Health:** keep `/health` reachable over the REST UDS so the SDK's readiness
  probe still works (it currently does an HTTP GET on `rest_url`).
- Unlink stale socket files on bind (a leftover socket from an unclean shutdown
  makes `bind()` fail with `EADDRINUSE`).
- Emit the per-query I/O trace as usual; UDS is a transport swap, not a new route.

## SDK changes (`clients/python/src/proximadb_sdk/`)

- **`embedded.py` (`EmbeddedConfig`/`EmbeddedProximaDB`)**: add the socket-dir
  convention (rules 1–4). Generate the server TOML with `transport = "uds"` +
  `socket_dir`. Expose `rest_uds_path` / `grpc_uds_path` / `flight_uds_path`.
- **REST over UDS**: `httpx` supports a UDS transport
  (`httpx.HTTPTransport(uds=path)` / `httpx.AsyncHTTPTransport(uds=path)`); pass a
  dummy `http://localhost` base URL and route the socket through the transport.
  This keeps the **generated REST client / ergonomic facade** intact (per CLAUDE.md
  mandate 15 — REST is spec-driven; only the transport swaps).
- **Arrow Flight over UDS**: `ArrowFlightClient` already parses `grpc+unix://`
  (pyarrow `flight.Location.for_grpc_unix(path)` — **verified working** with
  pyarrow 24). Point it at `grpc+unix://<flight.sock>`.
- **gRPC over UDS**: `grpc.aio` supports `unix:<path>` / `unix-abstract:` targets.
- Graph ops (REST) and vector ops (Flight) then both ride UDS with **zero TCP ports**.

## Victor alignment (already shaped — see below)

`victor/storage/proxima_runtime.py` `start_embedded_db` currently calls
`_pick_free_ports(2)`. Under UDS it should stop picking ports and let the SDK derive
sockets from the data dir. This handoff's companion change leaves a clear seam (a
`PROXIMADB_EMBEDDED_TRANSPORT` knob / TODO) so the flip is one edit once the SDK +
server land UDS. The gated tests stay skipped until then (by design — they
`pytest.skip` on any embedded-vector-path drift).

---

## Acceptance

1. An embedded instance starts with **no TCP port bound** (verify with `lsof`/`ss`:
   the proximadb-server child has only `unix` sockets, no `LISTEN` TCP).
2. The code-graph round-trip passes over UDS: `create_collection(dim=4)` →
   `insert_records` (Flight/UDS) → `search` returns the right oid; graph CRUD over
   REST/UDS.
3. Two embedded instances on **different data dirs** run concurrently with no
   collision and no port config.
4. Victor's two gated tests un-skip and pass live (the same ones in the parent
   handoff): `test_proxima_correlated_collection.py`, `test_proxima_embedded_parity.py`.
5. UDS path-length fallback exercised: an embedded instance under a deep
   `tmp_path` still starts (falls through to `$TMPDIR/proximadb-embedded-<uuid4>/`).
6. Mixed-read-safe: `transport = "tcp"` still works (default); UDS is opt-in.

## Prereqs already landed (this PR)

- **P0 catalog-registration fix**: embedded v2 collection create now actually
  registers in the catalog (was returning 200 while persisting nothing — GET showed
  `dimension:0`, LIST was empty). Portless vectors are pointless until a created
  collection is real, so that fix lands first, independent of transport.

---

## Paste-able command to start the portless-UDS session

```
Implement portless embedded mode in /Users/vijaysingh/code/proximaDB by binding the
embedded proximadb-server's REST, gRPC, and Arrow Flight surfaces to Unix-domain
sockets instead of TCP ports. Read roadmap/handoff/EMBEDDED_PORTLESS_UDS_HANDOFF.md
first — it has the locked decision, the exact socket-path convention (sockets under
<data_dir>/sockets/ with a $TMPDIR/<uuid4> fallback for object-store/over-long
paths), the server + SDK change map, the UDS path-length caveat, and the acceptance
list. Add `[api] transport = "tcp"|"uds"` (default tcp, opt-in, mixed-read-safe) +
`socket_dir`; serve tonic (gRPC + Flight) via serve_with_incoming over a
tokio UnixListener and axum REST over a UnixListener; wire the Python SDK's REST
(httpx uds transport), Flight (grpc+unix://, already supported), and gRPC (unix:
target) onto the sockets; then flip Victor's start_embedded_db off _pick_free_ports.
Acceptance: the code-graph round-trip runs with zero TCP ports, two instances on
different data dirs coexist, and Victor's two gated tests un-skip and pass live.
Work on a branch+worktree; no AI-attribution in commit/PR text.
```
