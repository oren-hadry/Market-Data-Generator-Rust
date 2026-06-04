mod books_manager;
mod book_side;
mod config;
mod engine;
mod generator;
mod order_book;
mod rate_controller;
mod sink;
mod time;
mod types;

use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use config::parse_config;
use engine::SimulationEngine;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: market_data <config.json>");
        std::process::exit(1);
    }

    let config = match parse_config(&args[1]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            std::process::exit(1);
        }
    };

    let mut engine = SimulationEngine::new(config);

    // Clone the running flag before start() so the ctrlc handler can set it.
    // C++ used a raw g_engine* pointer — Rust uses Arc: safe, no dangling reference possible.
    let running = engine.running_flag();

    ctrlc::set_handler(move || {
        eprintln!("\nStopping engine and flushing pipeline buffers...");
        running.store(false, Ordering::SeqCst);
    })
    .expect("failed to set Ctrl-C handler");

    engine.start();

    // Keep main alive — same role as `while(true) sleep_for(1s)` in C++.
    let running_main = engine.running_flag();
    while running_main.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(1));
    }
    // engine drops here → Drop::drop() calls stop() → joins threads
}
