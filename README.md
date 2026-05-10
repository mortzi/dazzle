# Dazzle

Market data aggregator for [Deribit](https://www.deribit.com/) — persistent WebSocket connection, L2 order book per instrument, REST and SSE HTTP API.

Written in Rust as a study of low-latency systems patterns. Connects to a live exchange, handles reconnection and message gaps correctly, and has measured latency numbers.

**Stack:** Rust · Tokio · Axum · tokio-tungstenite · DashMap

---

## What it does

- Persistent TLS WebSocket to Deribit with exponential backoff reconnection (500ms–30s, 30% jitter)
- L2 order book per instrument: full snapshot on subscribe → delta stream → `change_id` gap detection → automatic resubscribe
- Fixed-point `Price` and `Quantity` newtypes (`u64 × 10⁸`): no floats in the book, exact integer key ordering in the BTreeMap
- `walk_book`: simulates a market order sweeping through levels, returns average fill price and unfilled quantity; notional accumulated in `u128` to avoid overflow on large positions
- Ref-counted Deribit subscriptions — one WebSocket channel per instrument, shared across all HTTP consumers
- SSE streams: each connection gets a `SubscriptionStream` that filters a shared broadcast channel; broadcast lag is returned as an error and triggers resubscription rather than silent drop

---

## Architecture

```
HTTP Clients (REST / SSE)
        │
        ▼
axum Router
AppState — DashMap<Channel, Arc<BookManager>>
        │
        ▼
DeribitClient
  dispatch_loop task
  DashMap<id, oneshot::Sender>   ← pending RPC requests
  broadcast::Sender<Ticker>
  broadcast::Sender<OrderBookUpdate>
        │ mpsc
        ▼
ConnectionManager / ConnectionRunner
  WebSocket + exponential backoff
  heartbeat (30s interval, 45s read timeout)
        │ TLS WebSocket
        ▼
  Deribit Exchange
```

**BookManager** — one per active instrument. Background task holds `RwLock::write` during delta application; REST and SSE readers take `RwLock::read` and clone. On `Drop`, the background task is aborted.

**Order book lifecycle:**

```
maintain_book_state
  └─► subscribe_order_book
        ├─ Snapshot     → Book::from_snapshot, signal snapshot_ready
        ├─ Change       → check prev_change_id
        │                   mismatch → resubscribe (gap)
        │                   ok       → apply_level, broadcast Book
        ├─ ConnectionLost → resubscribe
        └─ Lag error      → resubscribe
```

---

## Design decisions

**Fixed-point arithmetic** — `BTreeMap` requires `Ord`. Float keys are unsound (NaN is unordered, denormals can compare unexpectedly). Storing prices as `u64 × 10⁸` makes key comparison a single integer subtract and eliminates rounding non-determinism on the update path.

**BTreeMap for the book** — gives sorted iteration for free: `best_bid` is `bids.iter().next_back()`, `best_ask` is `asks.iter().next()`, both O(log n). The natural alternative for a futures instrument with a fixed tick size would be a price-indexed array (`index = price / tick_size`), which gives O(1) access and contiguous memory layout. That doesn't work well for options books where hundreds of strikes land at arbitrary prices, making the array either sparse or unbounded. BTreeMap handles both instrument types with the same code and acceptable constants (5 ns for best bid/ask at any depth).

**Ref-counted subscriptions** — `DashMap<Channel, Mutex<u32>>` tracks consumers per channel. The tokio `Mutex` (not `std::sync::Mutex`) is held across the subscribe RPC call to serialize concurrent subscribe attempts to the same channel. Contention is per-channel so parallel subscriptions to different instruments don't block each other.

---

## Performance

Release build (`cargo build --release`).

### Micro-benchmarks (`cargo bench`)

| Benchmark | depth=25 | depth=100 | depth=500 |
|-----------|----------|-----------|-----------|
| `from_snapshot` | 378 ns | 973 ns | 4.53 µs |
| `update_mix` (80% insert, 20% delete) | 4.3 ns | 4.6 ns | 5.6 ns |
| `walk_book` | 48 ns | 142 ns | 570 ns |
| `book_serialize` | 1.77 µs | 6.30 µs | 29.7 µs |

| | latency |
|--|---------|
| `price_from_f64` | 410 ps |
| `best_bid` / `best_ask` | 3.0 ns / 1.8 ns |
| `spread` / `mid_price` | 3.9 ns |

`update_mix` is flat across depth because each delta is an independent O(log n) operation with a small constant. Serialization at depth=500 (30 µs) dominates per-push cost for large options books.

### Load test (`cargo run --example load_test`)

**Scenario 1: Update throughput** — 100k updates, realistic insert/delete mix, no readers:

| depth | throughput | p50 | p99 |
|-------|-----------|-----|-----|
| 25 | 7.4 M/s | 83 ns | 125 ns |
| 100 | 9.4 M/s | 42 ns | 84 ns |
| 500 | 11 M/s | 42 ns | 84 ns |

**Scenario 2: Read latency under write contention** — 10 concurrent readers cloning the book, 1 writer, depth=100, 2s:

| p50 | p95 | p99 | p999 |
|-----|-----|-----|------|
| 500 ns | 917 ns | 4.7 µs | 19 µs |

p50 is the clone cost. The gap to p99 is time waiting for the writer to release the lock.

**Scenario 3: `walk_book` latency** — 50k calls per depth, no concurrent writes:

| depth | p50 | p99 |
|-------|-----|-----|
| 25 | 83 ns | 84 ns |
| 100 | 166 ns | 209 ns |
| 500 | 583 ns | 792 ns |

Scales linearly with depth — `walk_book` iterates levels until the requested quantity is filled.

---

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/instruments?currency=BTC&kind=future` | List instruments |
| `GET` | `/ticker/{instrument}` | Ticker snapshot |
| `GET` | `/ticker/{instrument}/stream` | SSE ticker stream |
| `POST` | `/start-book/{instrument}` | Pre-warm a book |
| `POST` | `/stop-book/{instrument}` | Stop and evict a book |
| `GET` | `/book/{instrument}` | Book snapshot (auto-starts if needed) |
| `GET` | `/book/{instrument}/stream` | SSE book stream |

---

## Setup and running

```bash
cargo run
cargo test            # 27 tests: book mechanics, fixed-point, gap detection, stream behavior
cargo bench
cargo run --example simple_book    # fetch BTC-PERPETUAL and ETH-PERPETUAL snapshots
cargo run --example multi_client   # 10 concurrent ticker + SSE clients
cargo run --example load_test      # throughput and latency benchmarks
```

Environment variables:

```
SERVICE_HOST=0.0.0.0
SERVICE_PORT=3000
DERIBIT_URL=wss://test.deribit.com/ws/api/v2
RUST_LOG=info
```

---

## Limitations

- **L2 order book only** — Deribit's public API provides aggregated price levels, not individual order IDs. Queue position and order-level flow analysis require L3 data, which needs exchange co-location or a private feed.
- **Serialization on the hot path** — JSON serialization at 35 µs per book (depth=500) is the binding constraint on SSE push rate. A binary protocol would remove this.
- **BTreeMap allocation per insert** — each new price level allocates a tree node. For a futures instrument with a stable, known tick size, a pre-allocated price-indexed array would give O(1) updates and better cache locality.
