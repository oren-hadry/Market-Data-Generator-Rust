use std::collections::HashMap;

use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::Rng;
use rand_distr::{Distribution, Normal};

use crate::time::now_micros;
use crate::types::{Config, MarketEvent, Side, Symbol, BOOK_LEVELS};

// Rust trait = C++ abstract base class (pure virtual).
// `Send` supertrait means: "this type is safe to move to another thread" — required since
// the generator runs in the producer thread.
pub trait InputGenerator: Send {
    fn generate_next(&mut self) -> MarketEvent;
}

pub struct SyntheticGenerator {
    symbols: Vec<Symbol>,
    mid_prices: HashMap<Symbol, f64>,
    id_counters: HashMap<Symbol, [u64; 2]>,
    rng: StdRng,
}

impl SyntheticGenerator {
    pub fn new(config: &Config) -> Self {
        let mut id_counters = HashMap::new();
        for &sym in &config.symbols {
            id_counters.insert(sym, [0u64; 2]);
        }
        Self {
            symbols: config.symbols.clone(),
            mid_prices: config.initial_mid_prices.clone(),
            id_counters,
            rng: StdRng::seed_from_u64(1337),
        }
    }
}

impl InputGenerator for SyntheticGenerator {
    fn generate_next(&mut self) -> MarketEvent {
        let sym_idx = self.rng.gen_range(0..self.symbols.len());
        let symbol = self.symbols[sym_idx];

        let mid = self.mid_prices[&symbol];
        // Normal distribution for price walk — rand_distr::Normal replaces std::normal_distribution
        let dist = Normal::new(0.0, mid * 0.0001).unwrap();
        let delta: f64 = dist.sample(&mut self.rng);
        let new_mid = (mid + delta).max(0.0001);
        *self.mid_prices.get_mut(&symbol).unwrap() = new_mid;

        let side = if self.rng.gen_bool(0.5) { Side::Bid } else { Side::Ask };
        let side_int = if side == Side::Bid { 0 } else { 1 };

        let spread = new_mid * 0.0005;
        let price = if side == Side::Bid { new_mid - spread } else { new_mid + spread };

        let size = self.rng.gen_range(0.1f64..5.0f64);

        let counter = &mut self.id_counters.get_mut(&symbol).unwrap()[side_int];
        *counter = (*counter + 1) % BOOK_LEVELS as u64;
        let id = *counter;

        MarketEvent { ts: now_micros(), symbol, side, price, size, id }
    }
}

// Factory — mirrors make_input_generator() in C++.
// Returns Box<dyn InputGenerator>: heap-allocated trait object (= unique_ptr<InputGenerator>).
pub fn make_input_generator(config: &Config) -> Box<dyn InputGenerator> {
    match config.data_mode.as_str() {
        "synthetic" => Box::new(SyntheticGenerator::new(config)),
        mode => panic!("unsupported data_mode: {mode}"),
    }
}
