# Architecture

## Component Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         HTTP Clients                            │
└───────┬─────────────────────────────────────┬───────────────────┘
        │ REST                                │ SSE
        ▼                                     ▼
┌───────────────────────────────────────────────────────────────┐
│                      axum Router                              │
│                      AppState                                 │
│          DashMap<Channel, Arc<BookManager>>                   │
└───────────────────────┬───────────────────────────────────────┘
                        │
            ┌───────────▼───────────┐
            │     DeribitClient     │
            │                       │
            │  dispatch_loop task   │
            │  pending_requests     │
            │  DashMap<Channel,     │
            │    Mutex<u32>>        │
            │  broadcast::Sender    │
            │    <Ticker>           │
            │  broadcast::Sender    │
            │    <OrderBookUpdate>  │
            └───────────┬───────────┘
                        │ mpsc
            ┌───────────▼───────────┐
            │   ConnectionManager   │
            │   ConnectionRunner    │
            │                       │
            │  WebSocket + backoff  │
            │  heartbeat handling   │
            └───────────┬───────────┘
                        │ TLS WebSocket
                        ▼
                 Deribit Exchange
```

## Data Flow

### Inbound WebSocket message

```
Deribit WS frame
  └─► ConnectionRunner::handle_connection
        ├─ Message::Text  ──► mpsc → DeribitClient::dispatch_loop
        ├─ Message::Ping  ──► Pong (handled inline)
        └─ disconnect     ──► InboundMessage::ConnectionLost → mpsc
```

```
DeribitClient::dispatch_loop
  ├─ has "id" field  ──► oneshot to pending_requests (request/response)
  ├─ method = "subscription"
  │    ├─ channel starts "ticker" ──► broadcast::Sender<Ticker>
  │    └─ channel starts "book"   ──► broadcast::Sender<OrderBookUpdateMessage>
  ├─ method = "heartbeat/test_request" ──► send public/test pong
  └─ InboundMessage::ConnectionLost ──► broadcast OrderBookUpdateMessage::ConnectionLost
```

### Order book lifecycle

```
BookManager::maintain_book_state (tokio task)
  └─► subscribe_order_book → SubscriptionStream<OrderBookUpdateMessage>
        ├─ Snapshot  ──► Book::from_snapshot
        │                snapshot_tx.send(true)   ← unblocks wait_for_snapshot
        │                backoff.reset()
        ├─ Change    ──► check prev_change_id
        │                  mismatch? ──► resubscribe (gap detected)
        │                  ok?       ──► apply_level × n levels
        │                               broadcast Book to SSE consumers
        ├─ ConnectionLost ──► resubscribe
        └─ Lag error      ──► resubscribe
```

### HTTP request paths

```
GET /book/{instrument}
  └─► AppState::get_or_create_book_manager
        └─► BookManager::wait_for_snapshot   (watch::Receiver<bool> — blocks until ready)
              └─► book.read().await.clone()   (RwLock read + full clone)

GET /book/{instrument}/stream
  └─► AppState::get_or_create_book_manager
        └─► BookManager::subscribe_book
              └─► SubscriptionStream<Book>    (OnDrop::KeepAlive — manager persists)
                    └─► axum Sse stream

GET /ticker/{instrument}/stream
  └─► DeribitClient::subscribe_ticker
        └─► SubscriptionStream<Ticker>        (OnDrop::Unsubscribe — ref-counted)
              └─► axum Sse stream
```

## Key Components

### ConnectionManager / ConnectionRunner

Owns the WebSocket. `ConnectionRunner::maintain_connection` loops forever: connect, handle messages, reconnect on failure with exponential backoff (500ms–30s with 30% jitter). A 45-second read timeout detects silent disconnects (15s buffer after the 30s heartbeat interval). On disconnect, sends `InboundMessage::ConnectionLost` upstream before retrying.

`ConnectionManager::connect` blocks until the first successful connection or 10s timeout, so the rest of the app starts with a live socket.

### DeribitClient

Single instance shared via `Arc`. Owns:
- **`dispatch_loop` task** — routes inbound messages. Responses matched by `id` field via `DashMap<u64, oneshot::Sender>`. Subscription pushes routed to broadcast channels by channel prefix.
- **`pending_requests`** — `scopeguard` cleans up the oneshot sender if the future is dropped before the response arrives.
- **`subscribed_channels: DashMap<Channel, Arc<Mutex<u32>>>`** — ref-counted subscriptions. `tokio::sync::Mutex` is held across the subscribe network call so concurrent subscribe attempts on the same channel are serialized — only one request is sent to Deribit. Lock is per-channel, so different instruments have independent mutexes.

### SubscriptionStream

Wraps `BroadcastStream<T>` with a filter closure and `OnDrop` behaviour:
- `OnDrop::Unsubscribe` — spawns a task to decrement the ref count and send `public/unsubscribe` when the count reaches zero. Used by ticker streams (HTTP connection drives lifetime).
- `OnDrop::KeepAlive` — no action on drop. Used by book SSE streams since the `BookManager` lifetime is independent of individual HTTP connections.

Lag is returned as `Err(AppError)` rather than silently skipped. For order book streams, `BookManager` treats this as a stream error and resubscribes.

### BookManager

One per instrument, stored in `AppState::order_book_managers`. Owns:
- **`Arc<RwLock<Book>>`** — shared between the update task (write) and `get_book` callers (read + clone).
- **`broadcast::Sender<Book>`** — sends a full book clone after every applied delta, but only if `receiver_count() > 0`.
- **`watch::Sender<bool>`** — signals snapshot readiness. `wait_for_snapshot` blocks on `rx.wait_for(|v| *v)` with a configurable timeout. Once `true`, never reset — a reconnect produces a new snapshot which overwrites the book state directly.

On `Drop`, the background task is aborted via `task.abort()`.

### Book

`BTreeMap<Price, Quantity>` for both sides. Asks iterate ascending (lowest ask first); bids iterate in reverse (highest bid first). Keys are `Price(u64)` newtypes — fixed-point with `1e8` scale — so price ordering is exact and hash/comparison are integer operations.

`apply_level` handles `"new"`, `"change"` (both map to `insert`), and `"delete"` (`remove`). `walk_book` simulates market order fills by consuming levels in price-priority order, accumulating notional in `u128` to avoid overflow (`price_raw × qty_raw` can exceed `u64` for large BTC positions).

## Concurrency Model

| Component | Shared via | Access pattern |
|-----------|-----------|----------------|
| `DeribitClient` | `Arc` | All methods `&self`; internal state uses `DashMap` + `AtomicU64` |
| `BookManager` | `Arc` | Update task holds `RwLock::write`; readers hold `RwLock::read` + clone |
| WebSocket sender | `mpsc::Sender` | Cloned freely; backpressure via bounded channel (32) |
| Subscription count | `Mutex<u32>` per channel | Held across network call to prevent double-subscribe |
| Book broadcast | `broadcast::Sender<Book>` | Cloned for each subscriber; lag returned as error |
