use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crossbeam_channel::bounded;

use crate::books_manager::BooksManager;
use crate::generator::make_input_generator;
use crate::rate_controller::RateController;
use crate::sink::{Sink, StdoutSink};
use crate::types::{Config, QuoteUpdate};

// Same capacity reasoning as C++: 10k qps → 100µs/event, 10ms OS jitter → 100 burst → 256 headroom.
const QUEUE_CAPACITY: usize = 256;

pub struct SimulationEngine {
    config: Config,
    // Arc<AtomicBool> = shared ownership of the running flag across threads.
    // C++ used `std::atomic<bool> running_` owned by the engine, with raw pointer in signal handler.
    // Rust Arc makes shared ownership explicit and safe — no dangling pointer possible.
    running: Arc<AtomicBool>,
    producer: Option<JoinHandle<()>>,
    consumer: Option<JoinHandle<()>>,
}

impl SimulationEngine {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            producer: None,
            consumer: None,
        }
    }

    // Returns a clone of the running flag so the signal handler (ctrlc) can stop the engine.
    pub fn running_flag(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        println!("ts,sym,side,price,size,id");
        eprintln!("Engine Initialization Successful. Press [CTRL+C] to terminate.");

        // crossbeam bounded channel replaces BoundedSPSCQueue.
        // Capacity = 256. When full, sender blocks (same backpressure as C++ spin-wait).
        let (tx, rx) = bounded::<QuoteUpdate>(QUEUE_CAPACITY);

        let config_p = self.config.clone();
        let config_c = self.config.clone();
        let running_p = self.running.clone();
        let running_c = self.running.clone();

        // Producer thread: mirrors producer_loop() in C++.
        // `move` closure captures by value — Rust's way of transferring ownership into a thread.
        // C++ used std::thread with a member function pointer; Rust uses closures.
        let producer = thread::spawn(move || {
            let mut generator = make_input_generator(&config_p);
            let mut books = BooksManager::new();
            // tx moved into books — BooksManager clones it for each OrderBook → each BookSide.
            books.populate_and_init_books(&config_p, tx.clone());

            while running_p.load(Ordering::Relaxed) {
                let event = generator.generate_next();
                books.process_event(&event);
            }
            // tx drops here → channel closes → receiver will drain then stop
        });

        // Consumer thread: mirrors OutputSink::run() in C++.
        let consumer = thread::spawn(move || {
            let mut rate = RateController::new(
                config_c.rate_profile,
                config_c.base_rate,
                config_c.max_rate,
            );
            // Box<dyn Sink> = std::unique_ptr<ISink>. Vec = std::vector.
            let mut sinks: Vec<Box<dyn Sink>> = vec![Box::new(StdoutSink)];

            loop {
                match rx.try_recv() {
                    Ok(q) => {
                        let t0 = Instant::now();
                        for sink in &mut sinks {
                            sink.consume(&q);
                        }
                        let elapsed = t0.elapsed();
                        let required = rate.sleep_duration();
                        if required > elapsed {
                            thread::sleep(required - elapsed);
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        if !running_c.load(Ordering::Relaxed) {
                            break;
                        }
                        std::hint::spin_loop(); // CPU_PAUSE() equivalent
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                }
            }

            // Drain remaining — mirrors the drain loop after stop() in C++.
            while let Ok(q) = rx.try_recv() {
                for sink in &mut sinks {
                    sink.consume(&q);
                }
            }
        });

        self.producer = Some(producer);
        self.consumer = Some(consumer);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Option::take() extracts the JoinHandle so we can join without moving out of self.
        if let Some(t) = self.producer.take() {
            t.join().expect("producer thread panicked");
        }
        if let Some(t) = self.consumer.take() {
            t.join().expect("consumer thread panicked");
        }
    }
}

// Drop trait = C++ destructor. Called automatically when engine goes out of scope.
impl Drop for SimulationEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
