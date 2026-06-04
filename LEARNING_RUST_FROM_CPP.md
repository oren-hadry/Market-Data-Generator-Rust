# Learning Rust — From C++ to Rust

A practical guide anchored to **this project's code**. Every concept maps to a file you can open right now.

---

## 1. Ownership — the biggest mental shift

In C++ you manage memory manually (or via RAII). In Rust the compiler enforces RAII automatically through **ownership rules**:

- Every value has exactly **one owner**
- When the owner goes out of scope, the value is dropped (destructor called)
- You can **move** ownership or **borrow** it temporarily

### C++ (this project)
```cpp
// simulation_engine.cpp
SimulationEngine engine(config);  // engine owns everything
engine.start();                   // spawns threads — engine still owns them
// engine goes out of scope → destructor joins threads
```

### Rust (this project — `src/engine.rs`)
```rust
let mut engine = SimulationEngine::new(config);
engine.start();
// engine goes out of scope → Drop::drop() calls stop() → joins threads
```

**Identical behavior. But in Rust, forgetting to join threads is a compile error if you try to share non-Send data. In C++ it's silent UB.**

---

## 2. Move semantics

In C++, move semantics are opt-in (`std::move`). In Rust, **everything is move by default**.

### C++ vs Rust

```cpp
// C++: copy by default, move is explicit
std::string a = "hello";
std::string b = std::move(a);  // a is now empty (but still valid)
```

```rust
// Rust: move by default — no copy unless you call .clone()
let a = String::from("hello");
let b = a;          // a is MOVED — using a after this is a compile error
let c = b.clone();  // explicit copy
```

### In this project (`src/engine.rs`)

```rust
let config_p = self.config.clone();   // explicit copy for producer thread
let config_c = self.config.clone();   // explicit copy for consumer thread

let producer = thread::spawn(move || {
    // `move` transfers ownership of config_p, running_p, tx into this closure
    // trying to use config_p after this line = compile error
});
```

The `move` keyword on a closure = the Rust equivalent of capturing by value in a C++ lambda `[=]`, but the compiler tracks exactly what was moved and prevents double-use.

---

## 3. Borrowing — references without pointers

Rust has references (`&T` and `&mut T`) but **no raw pointers in safe code**.

Rules:
- Any number of `&T` (shared/read-only) references at once, OR
- Exactly ONE `&mut T` (exclusive/write) reference at once
- Never both at the same time

```rust
// src/books_manager.rs
pub fn process_event(&mut self, event: &MarketEvent) {
    //                 ^mut self = exclusive access    ^&T = read-only borrow
    if let Some(book) = self.books.get_mut(&event.symbol) {
        book.handle_event(event.side, event.id, event.price, event.size, event.ts);
    }
}
```

**C++ equivalent:** `void process_event(const MarketEvent& event)` on a non-const method. In Rust, the compiler enforces this at every call site — no accidental aliased mutation.

---

## 4. Traits — abstract interfaces

Rust `trait` = C++ pure virtual class. But:
- No inheritance (no `class Derived : public Base`)
- Implemented separately from the struct definition
- Compiler resolves dispatch at monomorphization (like templates) or via vtable (`dyn Trait`)

### C++ (this project)
```cpp
// sink.hpp
class ISink {
public:
    virtual ~ISink() = default;
    virtual void consume(const QuoteUpdate& q) = 0;
};

class StdoutSink : public ISink {
public:
    void consume(const QuoteUpdate& q) override;
};
```

### Rust (this project — `src/sink.rs`)
```rust
pub trait Sink: Send {
    fn consume(&mut self, q: &QuoteUpdate);
}

pub struct StdoutSink;

impl Sink for StdoutSink {     // implementation is separate from the struct
    fn consume(&mut self, q: &QuoteUpdate) {
        println!("{},{},{},{:.5},{:.4},{}", ...);
    }
}
```

### Static vs dynamic dispatch

```cpp
// C++: always dynamic dispatch via vtable for virtual functions
std::unique_ptr<ISink> sink = std::make_unique<StdoutSink>();
sink->consume(q);  // vtable lookup
```

```rust
// Rust: you choose
let sink: Box<dyn Sink> = Box::new(StdoutSink);  // dyn = vtable (same as C++ virtual)
sink.consume(&q);

// OR: static dispatch (compiler generates specialized code, zero overhead)
fn dispatch<S: Sink>(sink: &mut S, q: &QuoteUpdate) { sink.consume(q); }
```

In this project we use `Box<dyn Sink>` in `engine.rs` — equivalent to C++ `unique_ptr<ISink>`.

---

## 5. Option and Result — no null, no exceptions

C++ uses:
- `nullptr` for missing values → potential segfault
- Exceptions for errors → invisible control flow

Rust uses:
- `Option<T>` = value present (`Some(T)`) or absent (`None`)
- `Result<T, E>` = success (`Ok(T)`) or error (`Err(E)`)

The compiler forces you to handle both cases.

### Option (this project — `src/engine.rs`)
```rust
// Option::take() extracts the JoinHandle, leaving None in its place
if let Some(t) = self.producer.take() {
    t.join().expect("producer panicked");
}
// if producer is None (not started), the if block is skipped — no null check needed
```

C++ equivalent:
```cpp
if (producer_thread_.joinable()) {
    producer_thread_.join();
}
```

### Result (this project — `src/config.rs`)
```rust
pub fn parse_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot open config: {e}"))?;
    //                                                    ^ ? = return Err early (like throw)
    ...
    Ok(Config { ... })
}
```

C++ equivalent:
```cpp
Config parse_config(const std::string& path) {
    std::ifstream f(path);
    if (!f) throw std::runtime_error("cannot open config: " + path);
    ...
    return cfg;
}
```

The `?` operator is syntactic sugar for "if Err, return it; if Ok, unwrap the value". Like `throw` but explicit at every call site.

---

## 6. Pattern matching — match is more than switch

`match` in Rust is like C++ `switch` but:
- Works on any type (enums, structs, tuples, ranges)
- Must be exhaustive — compiler errors if you miss a case
- Can destructure and bind values in one step

### In this project (`src/engine.rs`)
```rust
match rx.try_recv() {
    Ok(q) => {
        // process the quote update
    }
    Err(crossbeam_channel::TryRecvError::Empty) => {
        if !running_c.load(Ordering::Relaxed) { break; }
        std::hint::spin_loop();
    }
    Err(crossbeam_channel::TryRecvError::Disconnected) => break,
}
```

C++ equivalent with `if/else` and manual enum checks. The Rust compiler would reject this if you forgot the `Disconnected` arm.

### Enum pattern matching (`src/book_side.rs`)
```rust
let key = match self.side {
    Side::Bid => *self.levels.keys().next().unwrap(),      // lowest price
    Side::Ask => *self.levels.keys().next_back().unwrap(), // highest price
};
```

---

## 7. Closures + thread::spawn

C++ lambdas and Rust closures are similar, but Rust's capture semantics are enforced by the compiler.

### C++ (this project)
```cpp
auto push_fn = [this](const QuoteUpdate& q) {
    while (!output_queue_.try_push(q)) CPU_PAUSE();
};
```
Captures `this` by reference — if the engine is destroyed while the lambda is still alive, UB.

### Rust (this project — `src/engine.rs`)
```rust
let producer = thread::spawn(move || {
    // move = take ownership of everything captured
    // trying to use a non-Send type here = compile error
    while running_p.load(Ordering::Relaxed) {
        let event = generator.generate_next();
        books.process_event(&event);
    }
});
```

`thread::spawn` requires the closure to be `'static` (no borrowed references that could dangle) and `Send` (safe to send to another thread). The compiler enforces this — you cannot accidentally capture a `&T` reference that outlives the thread.

---

## 8. Arc + AtomicBool — shared state between threads

### C++ (original project)
```cpp
// global raw pointer — signal handler stores it
static SimulationEngine* g_engine = nullptr;

static void handle_sigint(int) {
    if (g_engine) g_engine->stop();  // could dangle if engine was destroyed
}
```

### Rust (this project — `src/engine.rs` + `src/main.rs`)
```rust
// Arc = atomically reference-counted pointer (like shared_ptr, but thread-safe)
let running: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

// Clone the Arc — increments ref count, shares the same allocation
let running_for_ctrlc = running.clone();

ctrlc::set_handler(move || {
    running_for_ctrlc.store(false, Ordering::SeqCst);
    // Arc guarantees the AtomicBool is alive as long as any Arc clone exists
});
```

**Rule of thumb:**
| C++ | Rust |
|-----|------|
| `std::shared_ptr<T>` | `Arc<T>` (thread-safe) |
| `std::unique_ptr<T>` | `Box<T>` |
| `std::atomic<bool>` | `AtomicBool` |
| `std::mutex` | `Mutex<T>` (wraps the data, not a lock) |

---

## 9. The BTreeMap / OrderedFloat trick

### Why `std::map<double>` works in C++

C++ `std::map` uses `operator<` which is defined for `double` (IEEE 754 ordering, NaN excluded in practice).

### Why `BTreeMap<f64>` fails in Rust

Rust requires `Ord` trait (total ordering) for map keys. `f64` does **not** implement `Ord` because `NaN != NaN` — no total order exists.

### Fix (`src/book_side.rs`)
```rust
use ordered_float::OrderedFloat;

// OrderedFloat<f64> implements Ord by treating NaN as greater than everything
levels: BTreeMap<OrderedFloat<f64>, BookLevel>

// Insert
self.levels.insert(OrderedFloat(price), level);

// Access inner f64
let price: f64 = key.0;
```

This is Rust making a footgun explicit. In C++ you'd get NaN-related bugs silently; in Rust you're forced to decide what NaN means before the code compiles.

---

## 10. impl vs class — no inheritance

Rust has no class hierarchy. Instead:

```rust
// Define data
pub struct RateController {
    profile: RateProfile,
    base_rate: f64,
    ...
}

// Add methods — can be in a different file or crate
impl RateController {
    pub fn new(profile: RateProfile, base_rate: f64, max_rate: f64) -> Self { ... }
    pub fn sleep_duration(&mut self) -> Duration { ... }
}

// Add trait implementation
impl Drop for SimulationEngine {
    fn drop(&mut self) { self.stop(); }
}
```

`Self` in a `new` function = the type being constructed. Convention: `new` is the constructor name (not enforced, just idiomatic).

---

## 11. Cargo vs CMake

| CMake | Cargo |
|-------|-------|
| `CMakeLists.txt` | `Cargo.toml` |
| `cmake -B build && cmake --build build` | `cargo build` |
| `cmake --build build --config Release` | `cargo build --release` |
| `find_package(jsoncpp)` | add `serde_json = "1"` to `[dependencies]` |
| `target_link_libraries(...)` | automatic — cargo handles it |
| `./build/market_data config.json` | `cargo run -- config.json` |

No manual linker flags, no pkg-config, no brew prefix paths. Cargo downloads and compiles all dependencies.

---

## 12. The Send + Sync traits — thread safety in the type system

Two marker traits that the compiler checks automatically:

| Trait | Meaning | C++ equivalent |
|-------|---------|---------------|
| `Send` | Safe to move to another thread | "you remembered to not share it" |
| `Sync` | Safe to access from multiple threads simultaneously | "you remembered to add a mutex" |

In C++, thread safety is a comment or convention. In Rust it's enforced:

```rust
// This will NOT compile:
let data = Rc::new(42);  // Rc is NOT Send (not thread-safe ref counting)
thread::spawn(move || {
    println!("{}", data);  // ERROR: Rc cannot be sent between threads safely
});

// This compiles:
let data = Arc::new(42);  // Arc IS Send (atomic ref counting)
thread::spawn(move || {
    println!("{}", data);  // OK
});
```

In this project, every type that crosses a thread boundary (`InputGenerator`, `Sink`, `Arc<AtomicBool>`) is required to be `Send`. The compiler verifies this at `thread::spawn`.

---

## Learning Path

1. **Read** [The Rust Book](https://doc.rust-lang.org/book/) — free online, chapters 4 (ownership), 10 (traits), 16 (concurrency) are the most relevant
2. **Open** `src/book_side.rs` — study the BTreeMap + Sender pattern
3. **Open** `src/engine.rs` — study Arc, thread::spawn, move closures, Drop
4. **Open** `src/config.rs` — study Result and the `?` operator
5. **Modify** `src/sink.rs` — add a `FileSink` that writes to a file instead of stdout
6. **Add** a second symbol in config and trace the event flow through all files

The best way to learn Rust is to let the compiler teach you — write something, read the error, fix it. The errors are intentionally descriptive.
