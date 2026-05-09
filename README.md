# Dazzle

Real-time market data aggregator for [Deribit](https://www.deribit.com/) — maintains live L2 order books and streams tickers and book snapshots over HTTP SSE.

## Overview

Dazzle connects to Deribit over a single persistent WebSocket, applies incremental book deltas with gap detection, and fans out updates to any number of HTTP consumers via Server-Sent Events. Key design decisions:

- **Fixed-point arithmetic** — prices and quantities stored as `u64 × 10⁸` (8 decimal places). Avoids float comparison bugs in BTreeMap keys and rounding non-determinism on hot paths.
- **Ref-counted subscriptions** — a `DashMap<Channel, Mutex<u32>>` tracks consumer count per channel. The WebSocket subscription is opened on the first consumer and closed when the last disconnects. A `tokio::sync::Mutex` (not `std::sync::Mutex`) is held across the subscribe network call — this is intentional: it serializes concurrent subscribes to the same channel so only one request is sent to Deribit. Contention is per-channel, so parallel subscriptions to different instruments are unaffected.
- **Broadcast + filter** — a single `broadcast::Sender<T>` per data type. Each SSE connection gets a `SubscriptionStream` that filters by instrument, so no per-connection channels are needed upstream.
- **Gap detection** — `prev_change_id` is checked on every delta. A mismatch triggers immediate resubscription and re-snapshot.
- **Exponential backoff** — both the WebSocket connection and book subscription retry with jitter, capped at 30s.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/instruments?currency=BTC&kind=future` | List instruments |
| `GET` | `/ticker/{instrument}` | Snapshot ticker |
| `GET` | `/ticker/{instrument}/stream` | SSE ticker stream |
| `POST` | `/start-book/{instrument}` | Pre-warm an order book |
| `POST` | `/stop-book/{instrument}` | Stop and evict an order book |
| `GET` | `/book/{instrument}` | Snapshot order book (auto-starts if needed) |
| `GET` | `/book/{instrument}/stream` | SSE order book stream |

## Setup

```
SERVICE_HOST=0.0.0.0
SERVICE_PORT=3000
DERIBIT_URL=wss://test.deribit.com/ws/api/v2
RUST_LOG=info
```

Copy `.env.example` to `.env` and adjust `DERIBIT_URL` to `wss://www.deribit.com/ws/api/v2` for production.

## Running

```bash
cargo run
```

**Examples:**

```bash
cargo run --example simple_book    # fetch BTC-PERPETUAL and ETH-PERPETUAL books
cargo run --example multi_client   # 10 concurrent ticker + SSE stream clients
cargo run --example load_test      # throughput and latency benchmarks
```

## Tests

```bash
cargo test
```

27 tests across:
- `order_book::book` — fixed-point round-trip, `from_snapshot`, analytics (`best_bid`, `best_ask`, `spread`, `mid_price`), `walk_book` (single level, multi-level slippage, partial fill, sell side, empty book)
- `order_book::book_manager` — `apply_level` for `new`, `change`, `delete`, and delete-nonexistent
- `deribit::channel` — parsing, slicing, equality, display
- `deribit::subscription_stream` — filter, lag error propagation, stream-ends-on-sender-drop

## Performance

All numbers from release build (`cargo build --release`).

### Micro-benchmarks (`cargo bench`)

| Benchmark | depth=25 | depth=100 | depth=500 |
|-----------|----------|-----------|-----------|
| `from_snapshot` | 415 ns | 1.04 µs | 4.91 µs |
| `update_mix` (80% insert, 20% delete) | 4.9 ns | 5.1 ns | 6.3 ns |
| `walk_book` | 55 ns | 166 ns | 668 ns |
| `book_serialize` | 2.1 µs | 7.5 µs | 35 µs |

| Benchmark | latency |
|-----------|---------|
| `price_from_f64` | 531 ps |
| `best_bid` / `best_ask` | 4 ns / 2 ns |
| `spread` / `mid_price` | 5 ns |

`update_mix` is flat across depths — individual delta cost is O(log n) with a tiny constant. Serialization at depth=500 (35 µs) is the most expensive per SSE push on a large options book.

### Load test (`cargo run --example load_test`)

**Update throughput** — 100k updates, realistic mix of inserts and deletes:

| depth | throughput | p50 | p99 |
|-------|-----------|-----|-----|
| 25 | 7 M/s | 83 ns | 125 ns |
| 100 | 10 M/s | 42 ns | 84 ns |
| 500 | 11 M/s | 42 ns | 84 ns |

**Read latency under write contention** — 10 concurrent readers cloning the book, 1 writer, depth=100, 2s:

| p50 | p95 | p99 | p999 |
|-----|-----|-----|------|
| 500 ns | 1.2 µs | 12 µs | 26 µs |

**`walk_book` scaling** — 50k calls per depth:

| depth | p50 | p99 |
|-------|-----|-----|
| 25 | 83 ns | 125 ns |
| 100 | 167 ns | 250 ns |
| 500 | 625 ns | 875 ns |

### Profiling

```bash
cargo build --release --example load_test
samply record ./target/release/examples/load_test
# opens Firefox profiler at http://127.0.0.1:3001
```
