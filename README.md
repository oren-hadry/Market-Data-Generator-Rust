# Enki — Synthetic Market Data Simulator (Rust)

Generates a synthetic order-book feed for configurable symbols, rate-limits output, and streams CSV to stdout. Designed for low-to-mid throughput market data (10–10,000 qps) — **not an HFT engine**.

---

## Quick Start

**Dependencies**
```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Build & run**
```bash
./run.sh                              # uses config/config_burst.json
./run.sh config/config_burst.json    # explicit config
cargo run --release -- config/config_burst.json | grep BTCUSD
```

Ctrl+C for graceful shutdown.

Output format:
```
ts,sym,side,price,size,id
1717430400000000,BTCUSD,bid,64967.50000,2.3142,7
```

---

## Architecture

Phase 1: Two threads communicate via a bounded channel
```
InputGenerator (SyntheticGenerator)
    │  MarketEvent (sym, side, price, size, id)
    ▼
BooksManager  ──  O(1) symbol lookup (HashMap)
    │
    ▼
OrderBook  (per symbol)
    ├── BookSide [bid]  BTreeMap<OrderedFloat<f64>, BookLevel>  — sorted, max 100 levels
    └── BookSide [ask]  BTreeMap<OrderedFloat<f64>, BookLevel>  — prunes worst on overflow
    │
    │  QuoteUpdate (via crossbeam Sender)
    ▼
crossbeam_channel::bounded<QuoteUpdate, 256>   — blocks producer when full (natural backpressure)
    │
    ▼
Consumer thread
    ├── RateController  — sine / constant / burst / ramp profiles
    └── Vec<Box<dyn Sink>>
         └── StdoutSink  — CSV to stdout
```

### Threads

| Thread | Work |
|--------|------|
| Producer | `populate_and_init_books()` then continuous `generate_next()` loop |
| Consumer | drain channel → rate-limit sleep (configurable) → fan out to sinks |

### Key files

| File | Responsibility |
|------|---------------|
| `src/types.rs` | `Config`, `MarketEvent`, `QuoteUpdate`, `BookLevel`, `Side`, `RateProfile` |
| `src/config.rs` | serde_json-based JSON config loader |
| `src/generator.rs` | `InputGenerator` trait; `SyntheticGenerator` — random-walk prices, cycling 0–99 ids per sym/side |
| `src/book_side.rs` | Price-keyed order book half; `id_to_price` reverse map for O(1) id→price lookup |
| `src/order_book.rs` | Owns bid + ask `BookSide`; routes events |
| `src/books_manager.rs` | `HashMap<sym, OrderBook>`; routes `MarketEvent` |
| `src/rate_controller.rs` | Sleep duration calculator for sine / constant / burst / ramp profiles |
| `src/sink.rs` | `Sink` trait; `StdoutSink` CSV impl |
| `src/engine.rs` | Wires everything; owns threads + `Arc<AtomicBool>` running flag |

---

## Design Decisions

| Decision | Reason |
|----------|--------|
| `BTreeMap<OrderedFloat<f64>, BookLevel>` | `f64` has no `Ord` in Rust (NaN); `OrderedFloat` wraps it to add total ordering |
| `crossbeam_channel::bounded(256)` | Replaces hand-rolled SPSC queue; same backpressure semantics, battle-tested |
| `Arc<AtomicBool>` for running flag | Replaces raw `g_engine*` pointer in signal handler — shared ownership, no dangling reference |
| `Sender<QuoteUpdate>` as push fn | `Sender` is `Clone + Send`; each `BookSide` gets its own clone — no `shared_ptr` needed |
| `Box<dyn Sink>` | Equivalent to `std::unique_ptr<ISink>`; heap-allocated trait object with vtable |
| `move` closures in `thread::spawn` | Transfers ownership into thread — compiler enforces no dangling references |

**Not HFT.** 10,000 qps = 100µs/event budget. HFT operates 3–4 orders of magnitude faster and requires kernel-bypass (DPDK/RDMA), CPU pinning, and huge pages.

---

## Configuration (JSON)

```json
{
  "data_mode": "synthetic",
  "symbols": ["BTCUSD", "ETHUSD", "ETHBTC"],
  "rate_profile": "burst",
  "base_rate": 500.0,
  "max_rate": 10000.0,
  "initial_mid_prices": {
    "BTCUSD": 65000.0,
    "ETHUSD": 3400.0,
    "ETHBTC": 0.052
  }
}
```

| Field | Values |
|-------|--------|
| `rate_profile` | `sine` · `constant` · `burst` · `ramp` |
| `base_rate` | min quotes/sec (≥ 10) |
| `max_rate` | max quotes/sec (≤ 10,000) |
| `data_mode` | `synthetic` (recorded: phase 2) |

---

## Stress Testing: Gradually Escalating Quote Rate

> *"How would you devise a synthetic Market Maker that gradually escalates its quoting rate over time for stress testing purposes?"*

Use `rate_profile=ramp`. The `RateController` linearly interpolates from `base_rate` → `max_rate` over 60 seconds, then holds at `max_rate`:

```
rate(t) = base_rate + min(t / 60, 1.0) × (max_rate - base_rate)
```

**Example** (`base_rate=100`, `max_rate=10000`):

| time | rate |
|------|------|
| 0s | 100 qps |
| 15s | 2575 qps |
| 30s | 5050 qps |
| 60s | 10,000 qps |

This lets a downstream system (trading engine, data store, network stack) be observed as load increases continuously — failures surface at the exact rate threshold where the system breaks, not just "it worked at 1k but failed at 10k".

Config:
```json
{ "rate_profile": "ramp", "base_rate": 100, "max_rate": 10000 }
```

The `burst` profile (random spikes to `max_rate` at 5% probability) is complementary — tests recovery from sudden load, not sustained escalation.

---

## Design Trade-offs & FAQ

**Q: Won't the spin-wait burn 100% CPU when the channel is full?**
Yes, by design — minimizing latency over power. `crossbeam_channel` spins on `try_recv` when the channel is empty. Mitigation: fall back to `yield` or a short sleep after N failed spins if CPU budget is constrained.

**Q: `f64` as a map key — NaN/precision issues?**
The C++ version used `double` as a `std::map` key — a known anti-pattern. Rust refuses to compile bare `f64` as a `BTreeMap` key because `f64` has no total ordering (`NaN != NaN`). The fix is `OrderedFloat<f64>`, which adds a defined ordering for `NaN` as an edge case. Phase 2 will move to `i64` fixed-point ticks (multiply by 10⁵–10⁸), shifting comparisons from FP to integer ALU.

**Q: Per-symbol producer threads (Phase 2) would break the single channel — how?**
Each symbol thread gets its own `crossbeam::bounded` channel. The consumer multiplexes across them via round-robin `try_recv`. Never MPSC — each channel has exactly one sender, so there is no lock contention. See [PHASE2_ARCHITECTURE.md](PHASE2_ARCHITECTURE.md).

**Q: Why pace the sink instead of throttling the generator?**
Throttling the generator couples book logic to transport schedule. Full channel = natural backpressure. Sink-side pacing lets rate profiles swap at runtime without touching order book code.

---

## C++ vs Rust — What Changed

| C++ | Rust | Note |
|-----|------|------|
| `std::map<double, BookLevel>` | `BTreeMap<OrderedFloat<f64>, BookLevel>` | `f64` not `Ord` in Rust |
| Hand-rolled `BoundedSPSCQueue` | `crossbeam_channel::bounded(256)` | Same semantics, no unsafe code |
| `std::function<void(QuoteUpdate)>` | `Sender<QuoteUpdate>` | Clone-able, Send-safe |
| `std::unique_ptr<ISink>` | `Box<dyn Sink>` | Identical ownership model |
| Raw `g_engine*` in signal handler | `Arc<AtomicBool>` | Compiler-verified safe sharing |
| `std::atomic<bool>` + UB risk | `Arc<AtomicBool>` | Data race = compile error, not UB |
| `jsoncpp` | `serde_json` | |

## Phase 2 Improvements (planned)

See [PHASE2_ARCHITECTURE.md](PHASE2_ARCHITECTURE.md) in the C++ repo for the full design.

### Performance
- **Per-symbol producer threads** — one thread per symbol, one channel per symbol, round-robin consumer.
- **CPU pinning** — bind threads to isolated cores via `core_affinity` crate.

### Output sinks
- **ZMQ sink** — `zmq` crate, PUB socket, binary frames.
- **WebSocket sink** — `tokio-tungstenite`, JSON stream.
- **FlatBuffers encoding** — `flatbuffers` crate, zero-copy wire format.

### Input modes
- **Recorded replay** — replay historical tick data at configurable `time_scale`.
- **Invalid orders** — crossed books, zero-size, duplicate ids.
- **Correlated walks** — model cross-asset correlation (ETHBTC ≈ ETHUSD/BTCUSD).
