# Phase 2 Architecture — Enki Market Data Simulator (Rust)

---

## Overview

Phase 1 uses two threads and one bounded channel. Phase 2 parallelizes across symbols, adds multiple output sinks, and introduces a proper observability layer.

---

## Thread Model

### Phase 1 (current)

```
[Producer Thread]  →  crossbeam::bounded<256>  →  [Consumer Thread]
  all symbols                                        all sinks
```

### Phase 2

```
[Producer BTCUSD]  →  RateCtrl[BTC]  →  bounded<BTCUSD>  ─┐
[Producer ETHUSD]  →  RateCtrl[ETH]  →  bounded<ETHUSD>  ─┤─→  [Consumer]  →  GlobalCap  →  Sink fanout
[Producer ETHBTC]  →  RateCtrl[EBT]  →  bounded<ETHBTC>  ─┘   (round-robin)
```

One producer thread per symbol. One consumer multiplexes across N channels via round-robin `try_recv`. No MPSC lock contention — each channel has exactly one sender and one receiver.

**Two-level rate limiting** — see [Rate Limiting](#rate-limiting) section below.

| Thread | Count | Work |
|--------|-------|------|
| Producer | 1 per symbol | `generate_next()` → per-asset rate sleep → send to channel |
| Consumer | 1 | round-robin drain → global cap → fanout to sinks |
| Metrics  | 1 | collect counters, push to Prometheus or shared ring buffer |

---

## Data Flow

```
Config (JSON / serde_json)
    │
    ▼
SimulationEngine
    │
    ├── InputGeneratorPool
    │     ├── Generator[sym0]  →  RateController[sym0]  →  bounded<QuoteUpdate>[sym0]  ─┐
    │     ├── Generator[sym1]  →  RateController[sym1]  →  bounded<QuoteUpdate>[sym1]  ─┤
    │     └── Generator[symN]  →  RateController[symN]  →  bounded<QuoteUpdate>[symN]  ─┘
    │           │                                                                        │
    │     (SyntheticGenerator  ←── per-asset rate profile                               │
    │      RecordedGenerator   ←── real tick data, time-scaled)                         │
    │                                                                                    │
    ├── BooksManager                                                                     │
    │     └── OrderBook[sym]                                                             │
    │           ├── BookSide[bid]  ──→ QuoteUpdate via Sender ───────────────────────── ┘
    │           └── BookSide[ask]
    │
    └── Consumer thread (round-robin across channels)
          ├── GlobalRateCap   — hard ceiling for downstream capacity
          └── Vec<Box<dyn Sink>>
                ├── StdoutSink       — CSV (Phase 1 carry-over)
                ├── ZmqSink          — PUB socket via `zmq` crate
                ├── WebSocketSink    — JSON stream via `tokio-tungstenite`
                └── FlatBuffersSink  — zero-copy via `flatbuffers` crate
```

---

## Key Changes vs Phase 1

### 1. Per-Symbol Producer Threads

**Why:** At high symbol counts (50–500), a single producer serializes `generate_next()` across all symbols. Rust `thread::spawn` with `move` closures makes this natural — each thread owns its generator and books, zero shared state.

**Rust advantage over C++:** The compiler enforces that no data is shared across thread boundaries without `Arc` or `Mutex`. Per-symbol threads with no shared state between them compile cleanly — in C++ this would be a discipline check, not a compile check.

### 2. Multiple Channels (one per symbol)

One `crossbeam::bounded<QuoteUpdate>` per symbol. Consumer polls in round-robin:

```rust
let receivers: Vec<Receiver<QuoteUpdate>> = ...;
let mut idx = 0;
loop {
    if let Ok(q) = receivers[idx].try_recv() {
        // process q
    }
    idx = (idx + 1) % receivers.len();
}
```

If one symbol produces at high rate, it does not starve others — round-robin is fair.

### 3. Rate Limiting (Two-Level)

| Level | Where | Controls | Models |
|-------|-------|----------|--------|
| Per-asset rate | Producer thread | How fast each symbol generates events | Market conditions (BTCUSD can spike while ETHBTC is quiet) |
| Global output cap | Consumer thread | Max total events/sec to sinks | Downstream capacity (network, data store, subscriber) |

```
Producer[BTC]  →  RateController[BTC, burst]  →  bounded  ─┐
Producer[ETH]  →  RateController[ETH, sine]   →  bounded  ─┤→  Consumer  →  GlobalCap(50k/s)  →  Sinks
Producer[EBT]  →  RateController[EBT, const]  →  bounded  ─┘
```

Config example:
```json
{
  "symbols": [
    { "name": "BTCUSD", "rate_profile": "ramp",  "base_rate": 100,  "max_rate": 5000 },
    { "name": "ETHUSD", "rate_profile": "sine",  "base_rate": 200,  "max_rate": 3000 },
    { "name": "ETHBTC", "rate_profile": "const", "base_rate": 50,   "max_rate": 50   }
  ],
  "global_max_rate": 50000
}
```

### 4. CPU Pinning

Bind each producer thread and the consumer thread to isolated cores via the `core_affinity` crate:

```rust
core_affinity::set_for_current(core_affinity::CoreId { id: cpu_id });
```

Eliminates scheduler jitter on hot paths. Equivalent to `pthread_setaffinity_np` on Linux / `thread_policy_set` on macOS.

### 5. Output Sink Fanout (Parallel)

Phase 1 dispatches serially (`for sink in &mut sinks { sink.consume(&q) }`). Phase 2 gives each sink its own bounded channel + thread:

```
Consumer (dispatcher)
    │
    ├── send to sink_tx[0]  (StdoutSink thread)
    ├── send to sink_tx[1]  (ZmqSink thread)
    └── send to sink_tx[2]  (WebSocketSink thread)
```

One slow sink (e.g. WebSocket with TCP backpressure) does not block the others. Each sink thread drains its own `Receiver<QuoteUpdate>` independently.

**Rust advantage:** `Sender<T>` is `Clone + Send`. Cloning a sender for each sink thread is zero-unsafe. In C++ this pattern requires careful lifetime management of the queue reference.

### 6. Input Modes

| Mode | Struct | Description |
|------|--------|-------------|
| `synthetic` | `SyntheticGenerator` | Random-walk prices (Phase 1) |
| `recorded` | `RecordedGenerator` | Replay real historical tick data from file |
| `synthetic+invalid` | `InvalidOrderGenerator` | Inject crossed books, zero-size, dup ids |
| `synthetic+correlated` | `CorrelatedGenerator` | Model cross-asset correlation (ETHBTC ≈ ETHUSD/BTCUSD) |

#### Recorded Mode — Real Market Data

`RecordedGenerator` reads historical L2/tick data (Binance, Coinbase, Tardis.dev CSV) and replays it through the same `OrderBook` / `BookSide` pipeline:

```
tick_data/BTCUSD_2024-01-15.csv
    │  ts, side, price, size, id
    ▼
RecordedGenerator
    ├── reads next row (via csv crate or BufReader)
    ├── sleeps (next_ts - prev_ts) / time_scale
    └── emits MarketEvent → same pipeline as synthetic
```

**Use cases:**
- Replay a known market crash or liquidity crunch
- `time_scale=10` compresses a full trading day into minutes for regression testing
- Validate that `SyntheticGenerator` statistical properties match real distributions

### 7. Observability

A dedicated metrics thread samples atomic counters and writes to a shared ring buffer or pushes to Prometheus:

```rust
// Counter per channel
let enqueued = Arc::new(AtomicU64::new(0));
let consumed  = Arc::new(AtomicU64::new(0));
// Metrics thread reads and reports every second
```

| Metric | Source | Use |
|--------|--------|-----|
| Channel depth | `sender.len()` | Backpressure signal; tune capacity |
| Enqueue→emit latency | `Instant` stamp in QuoteUpdate | P99 latency budget |
| Dropped events | `enqueued - consumed` | Detect overload |
| Prune count per symbol | `AtomicU64` in `BookSide` | Hot-symbol detection |

### 8. Config Hot-Reload

```
SIGHUP  →  reload config file  →  update RateController params at runtime
```

`RateController` reads from `Arc<AtomicU64>` (fixed-point rates), so the signal handler can update them lock-free without touching the producer thread. In Rust, this is enforced by the type system — you cannot write to a non-atomic value from two threads.

---

## Performance Targets

| Metric | Phase 1 | Phase 2 (target) |
|--------|---------|-----------------|
| Max throughput | 10,000 qps | 500,000+ qps |
| Symbols | 3–10 | 50–500 |
| Latency (p99 enqueue→emit) | ~1ms | <50µs (pinned) |
| Output sinks | 1 (stdout) | N (ZMQ, WS, FlatBuffers) |
| CPU usage at max rate | 100% (spin-wait) | Bounded (pinned core) |

**Still not HFT.** HFT operates at <1µs with kernel-bypass (DPDK/RDMA) and custom NICs. Phase 2 targets mid-frequency data distribution: 100µs–1ms latency at high throughput.

---

## New Crates for Phase 2

| Crate | Replaces / Adds |
|-------|----------------|
| `core_affinity` | CPU pinning (`pthread_setaffinity_np`) |
| `zmq` | ZMQ PUB socket sink |
| `tokio` + `tokio-tungstenite` | Async WebSocket sink |
| `flatbuffers` | Zero-copy binary wire format |
| `csv` | RecordedGenerator tick file parsing |
| `prometheus` | Metrics exposition |

---

## What Does Not Change

- `BookSide` / `OrderBook` — unchanged; no thread boundary crosses book logic
- `RateController` — same profiles (sine, constant, burst, ramp), one per symbol
- `Sink` trait — all new sinks implement the same `consume(&QuoteUpdate)` API
- CSV stdout output — `StdoutSink` is still a valid sink in Phase 2
- Config JSON format — extended, not replaced
