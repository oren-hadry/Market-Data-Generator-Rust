# Migrating a C++ Systems Project to Rust

This document reflects on porting a synthetic market data simulator from C++ to Rust.
The C++ version is at [Enki — Market Data (C++)](https://github.com/oren-hadry/enki-market-data).
This repo is the Rust port.

The project is a two-thread producer/consumer pipeline: a generator pushes synthetic order-book
events through a bounded queue to a consumer that rate-limits and writes CSV to stdout.

---

## Why Port to Rust?

The C++ implementation is correct, but several safety properties rely on discipline rather than
compiler enforcement:

- The signal handler stores a raw `g_engine*` pointer that could dangle
- Thread closures capture `this` by reference with no lifetime verification
- `std::map<double>` as a price key relies on the assumption that NaN never appears
- Thread safety of the `running_` flag is documented, not enforced

Rust eliminates each of these by making them compile errors rather than runtime bugs.
The performance characteristics are identical — both compile to native code with no GC.

---

## Key Migrations

### 1. Raw pointer in signal handler → Arc

**C++**
```cpp
static SimulationEngine* g_engine = nullptr;

static void handle_sigint(int) {
    if (g_engine) g_engine->stop(); // dangling pointer if engine was destroyed first
}

int main() {
    SimulationEngine engine(config);
    g_engine = &engine;             // raw pointer — no lifetime tracking
    ...
}
```

**Rust** (`src/main.rs`, `src/engine.rs`)
```rust
let mut engine = SimulationEngine::new(config);
let running = engine.running_flag(); // returns Arc<AtomicBool> — clones the ref count

ctrlc::set_handler(move || {
    running.store(false, Ordering::SeqCst);
    // Arc guarantees the allocation lives as long as any clone exists
})?;
```

`Arc<T>` is reference-counted atomic shared ownership. The signal handler holds a clone of
the Arc — the underlying `AtomicBool` is guaranteed to be alive. No null check, no dangling
pointer, enforced by the type system.

---

### 2. Thread closures — capture discipline → compiler enforcement

**C++**
```cpp
// simulation_engine.cpp — captures `this` by reference
auto push_fn = [this](const QuoteUpdate& q) {
    while (!output_queue_.try_push(q)) CPU_PAUSE();
};
// If `engine` is destroyed while push_fn is in use: undefined behavior
```

**Rust** (`src/engine.rs`)
```rust
let producer = thread::spawn(move || {
    // `move` transfers ownership of generator, books, running_p into the thread
    // The compiler rejects any non-'static borrow crossing the thread boundary
    while running_p.load(Ordering::Relaxed) {
        books.process_event(&generator.generate_next());
    }
});
```

`thread::spawn` requires `'static + Send`. This makes it a **compile error** to capture a
borrowed reference that could outlive the thread. In C++, the same mistake is silent UB.

---

### 3. Hand-rolled SPSC queue → crossbeam bounded channel

**C++** — 60-line custom lock-free queue with `alignas(64)`, bitmask index, `CPU_PAUSE()`.

**Rust** — one line:
```rust
let (tx, rx) = crossbeam_channel::bounded::<QuoteUpdate>(256);
```

`crossbeam::bounded` is a battle-tested, lock-free bounded channel with identical semantics:
blocks the sender when full (natural backpressure), `try_recv` for spin-wait on the consumer.
`Sender<T>` is `Clone + Send` — multiple senders to the same channel without a mutex.

The custom C++ queue was necessary because the standard library lacks a bounded SPSC channel.
In Rust, `crossbeam-channel` is the de facto standard and is maintained by the concurrency
working group.

---

### 4. std::map\<double\> → BTreeMap\<OrderedFloat\<f64\>\>

**C++**
```cpp
std::map<double, BookLevel> levels_; // works — operator< is defined for double
```

**Rust** — this does not compile:
```rust
BTreeMap<f64, BookLevel> // ERROR: f64 does not implement Ord
```

Rust requires `Ord` (total ordering) for map keys. `f64` does not implement `Ord` because
`NaN != NaN` — floating-point has no total order. The C++ version assumes NaN never appears;
the Rust version forces an explicit decision:

```rust
use ordered_float::OrderedFloat;

BTreeMap<OrderedFloat<f64>, BookLevel>

// OrderedFloat<f64> implements Ord — NaN is treated as greater than any other value
self.levels.insert(OrderedFloat(price), BookLevel { size, id });
```

This surfaces a real assumption that existed silently in the C++ code.

---

### 5. ISink virtual class → Sink trait

**C++**
```cpp
class ISink {
public:
    virtual ~ISink() = default;
    virtual void consume(const QuoteUpdate& q) = 0;
};

class StdoutSink : public ISink {
    void consume(const QuoteUpdate& q) override;
};

std::vector<std::unique_ptr<ISink>> sinks_;
```

**Rust** (`src/sink.rs`, `src/engine.rs`)
```rust
pub trait Sink: Send {
    fn consume(&mut self, q: &QuoteUpdate);
}

pub struct StdoutSink;

impl Sink for StdoutSink {
    fn consume(&mut self, q: &QuoteUpdate) { println!("{},{},…", q.ts, q.symbol); }
}

let sinks: Vec<Box<dyn Sink>> = vec![Box::new(StdoutSink)];
```

`Box<dyn Sink>` = `unique_ptr<ISink>`. The `Send` supertrait means the compiler verifies
every `Sink` implementation is safe to send to the consumer thread — no annotation required
on each call site.

Traits differ from C++ virtual classes in one key way: **no inheritance**. A type can implement
multiple traits but cannot extend another struct. Shared behavior composes through traits, not
class hierarchies.

---

### 6. Exceptions and nulls → Result and Option

**C++**
```cpp
Config parse_config(const std::string& path) {
    std::ifstream f(path);
    if (!f) throw std::runtime_error("cannot open: " + path);
    ...
}
// caller may or may not catch — nothing in the signature says this throws
```

**Rust** (`src/config.rs`)
```rust
pub fn parse_config(path: &str) -> Result<Config, Box<dyn Error>> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot open {path}: {e}"))?;
    ...
    Ok(Config { ... })
}
// the signature advertises that this can fail — caller is forced to handle it
```

`Result<T, E>` makes fallibility part of the type signature. The `?` operator propagates
errors up the call stack (equivalent to `throw`), but it is **explicit at each site** — a
reader can see exactly where errors can originate.

`Option<T>` replaces nullable pointers:
```rust
// engine.rs — join without a null check
if let Some(t) = self.producer.take() {
    t.join().expect("producer panicked");
}
```
`Option::take()` extracts the value and leaves `None` in its place — same pattern as
`std::exchange(ptr, nullptr)` in C++, but enforced.

---

### 7. Thread safety — convention → type system

In C++, thread safety is documented. In Rust, it is enforced by two marker traits:

| Trait | Meaning |
|-------|---------|
| `Send` | Safe to transfer ownership to another thread |
| `Sync` | Safe to share a reference across threads simultaneously |

```rust
// Rc<T> is NOT Send — single-threaded reference counting
let data = Rc::new(42);
thread::spawn(move || println!("{data}")); // COMPILE ERROR

// Arc<T> IS Send — atomic reference counting
let data = Arc::new(42);
thread::spawn(move || println!("{data}")); // OK
```

Every type that crosses a thread boundary in this project — `InputGenerator`, `Sink`,
`Arc<AtomicBool>` — is verified `Send` by the compiler at `thread::spawn`. In C++, using
`std::shared_ptr` in a signal handler without synchronization is UB and compiles without warning.

---

## Ownership Model Comparison

| C++ concept | Rust equivalent | Difference |
|-------------|-----------------|------------|
| Stack variable (RAII) | Owned value | Rust tracks moves; C++ allows accidental copies |
| `std::unique_ptr<T>` | `Box<T>` | Identical ownership semantics |
| `std::shared_ptr<T>` | `Arc<T>` | Rust's `Arc` is always thread-safe; no `shared_ptr` equivalent for single-threaded (`Rc<T>`) |
| `const T&` | `&T` | Rust enforces: no `&mut T` coexists with any `&T` |
| `T&` | `&mut T` | Rust enforces: only one `&mut T` at a time |
| `virtual void f() = 0` | `fn f(&mut self)` in a `trait` | Rust traits have no inheritance |
| `[[nodiscard]]` + exceptions | `Result<T, E>` | Rust enforces handling at the type level |

---

## A Real Bug: Producer Initialized Before Consumer → Deadlock

During development, the engine's `start()` method was written to spawn the producer thread
before the consumer thread. The program hung immediately on every run.

**Root cause:**

`start()` was written to spawn the producer thread first, then the consumer thread.
The producer's first action is sending an initial order book snapshot — 100 price levels,
both sides, for every symbol. With 3 symbols: `100 × 2 × 3 = 600` events.

The channel capacity is 256. The producer blocks on the 257th send. Because the producer
thread is stuck inside `populate_and_init_books()`, `start()` never returns to the line
that spawns the consumer. The consumer is never created. Nobody drains the channel.

```
start() {
    spawn producer thread {
        populate_and_init_books()  // sends 600 events
            → channel full at 256
            → send() BLOCKS
            → thread stuck here forever
    }
    // ← never reached
    spawn consumer thread          // consumer never spawned
}
```

The fix: spawn the consumer thread first, then the producer.

```rust
// engine.rs — correct order
let consumer = thread::spawn(move || { /* drain channel */ });
let producer = thread::spawn(move || {
    books.populate_and_init_books(&config_p, tx); // sends 600 events
    loop { /* generate */ }
});
```

With the consumer already running, it drains as fast as the producer fills. The channel
never stays full for long and the snapshot completes without blocking.

**Does Rust prevent this?**

No. This is a design constraint, not a type error. Neither language can enforce
"consumer must be running before producer sends". The compiler has no model of
thread initialization order.

What Rust *did* do: make the failure **immediate and obvious**. A bounded channel
blocks on the 257th send. An unbounded queue would have let the producer run freely,
silently allocating memory until the process was killed or the machine ran out of RAM —
a much harder bug to diagnose.

The C++ implementation had the correct order:
```cpp
output_sink_.start();                                          // consumer first
producer_thread_ = std::thread(&SimulationEngine::producer_loop, this); // producer second
```

That ordering was lost during the port. The deadlock surfaced in the first test run.
Bounded queues fail fast — which is why they are the right default for production systems.

---

## What Rust Did Not Change

The performance-critical design decisions from the C++ version are preserved unchanged:

- **Bounded queue** — same capacity (256), same backpressure semantics
- **Rate profiles** — sine, constant, burst, ramp — identical math
- **Order book logic** — `BTreeMap` (sorted) + `HashMap` reverse map (O(1) id→price lookup)
- **Drain on shutdown** — consumer drains remaining events after `running` goes false
- **CSV output format** — identical wire format, same precision (`{:.5}` price, `{:.4}` size)

The port is a language change, not an architecture change.

---

## Summary

Porting this project from C++ to Rust required solving four concrete problems:

1. **Signal handler safety** — replaced raw pointer with `Arc`
2. **Thread closure safety** — `move` closures with compiler-verified `Send` bounds
3. **Map key correctness** — `OrderedFloat<f64>` forces an explicit decision about NaN
4. **Error propagation** — `Result<T, E>` makes fallibility visible in every function signature

In each case, Rust did not change what the code *does* — it changed what the compiler
*accepts*. The bugs that were previously possible but unlikely become impossible to express.
