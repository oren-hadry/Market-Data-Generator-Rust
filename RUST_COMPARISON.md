# C++ vs Rust — Design Mapping

## C++ patterns that translate directly to Rust

| C++ | Rust | Note |
|-----|------|------|
| `std::map<double, BookLevel>` | `BTreeMap<OrderedFloat<f64>, BookLevel>` | Both sorted; `OrderedFloat` adds total ordering `f64` lacks |
| `BoundedSPSCQueue<T, N>` (hand-rolled) | `crossbeam_channel::bounded(N)` | Same backpressure semantics; no `unsafe` needed |
| `std::function<void(QuoteUpdate)>` PushFn | `Sender<QuoteUpdate>` | `Sender` is `Clone + Send`; no `shared_ptr` needed |
| `std::unique_ptr<ISink>` | `Box<dyn Sink>` | Identical ownership model; vtable dispatch |
| `ISink` virtual interface | `trait Sink` | Same pattern; `dyn Sink` = vtable, `impl Sink` = monomorphized |
| `std::atomic<bool>` running flag | `Arc<AtomicBool>` | `Arc` replaces raw pointer sharing; same memory model (`Ordering::SeqCst`) |
| `alignas(64)` on atomics | `#[repr(align(64))]` | Cache-line padding — identical intent |
| `std::atomic` acquire/release | `Ordering::Acquire` / `Ordering::Release` | Identical memory model |
| Signal handler with `g_engine*` | `Arc<AtomicBool>` in signal handler | Rust enforces shared ownership; no dangling pointer possible |
| `jsoncpp` manual field extraction | `#[derive(Deserialize)]` + `serde_json` | ~30 lines of C++ → 1 derive macro |

---

## What Rust fixes

| C++ problem | Rust fix |
|-------------|---------|
| Two threads can corrupt SPSC — silent UB | `Send` + borrow checker — compile error, not runtime crash |
| `g_engine` raw global pointer in signal handler | `Arc<AtomicBool>` — shared ownership verified at compile time |
| `parse_config` throws — easy to miss | `Result<Config, Error>` — caller forced to handle the error |
| `double` as `std::map` key — NaN UB | `OrderedFloat<f64>` — total ordering enforced by type; NaN handled explicitly |
| Config parsing — 30 lines of manual JSON | `#[derive(Deserialize)]` + `serde_json::from_reader(f)?` |

**Biggest example:** the producer-before-consumer deadlock (`populate_and_init_books` called before `output_sink_.start()`) hung silently in C++ until discovered at runtime. In Rust, the bounded channel blocks the producer the moment it's full — the hang surfaces immediately in testing, and the fix (reordering thread startup) is obvious. The C++ version could run for seconds before deadlocking depending on queue capacity.

---

## Where C++ wins

- **Ecosystem** — every exchange SDK, FIX engine, ITCH/OUCH parser, and co-location library is C++. Rust bindings don't exist or lag by years.
- **Raw control** — `_mm_pause()`, manual cache-line layout, huge page allocation work naturally. Rust needs `unsafe {}` blocks and extra crates.
- **Compile times** — Rust is 3–5× slower to compile at scale. Painful during live market hours when you need a fast turnaround.
- **Hiring** — HFT shops and exchange infrastructure run on C++. The Rust talent pool in this domain is thin.

**Bottom line:** Rust wins on correctness — the compiler catches the class of bugs that cause silent data corruption or undefined behaviour in C++. C++ wins on ecosystem fit for anything touching exchange connectivity or co-location infrastructure. New isolated service or simulator → Rust. Integrating with existing market infra → C++.
