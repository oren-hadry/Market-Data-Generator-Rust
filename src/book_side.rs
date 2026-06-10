use std::collections::{BTreeMap, HashMap};

use crossbeam_channel::Sender;  // The project needs a bounded channel with multiple senders. std::sync::mpsc can't do both cleanly
use ordered_float::OrderedFloat;

use crate::time::now_micros;
use crate::types::{BookLevel, QuoteUpdate, Side, BOOK_LEVELS};

// Rust equivalent of BookSide::PushFn: Sender<QuoteUpdate> replaces std::function<void(QuoteUpdate)>.
// Sender is Clone + Send, so each BookSide (bid/ask) gets its own clone — no shared_ptr needed.
pub struct BookSide {
    side: Side,
    tx: Sender<QuoteUpdate>,
    // std::map<double, BookLevel> → BTreeMap<OrderedFloat<f64>, BookLevel>
    // BTreeMap is sorted (same as std::map). OrderedFloat wraps f64 to implement Ord,
    // since bare f64 has no total ordering in Rust (NaN != NaN).
    levels: BTreeMap<OrderedFloat<f64>, BookLevel>,
    id_to_price: HashMap<u64, f64>,
}

impl BookSide {
    pub fn new(side: Side, tx: Sender<QuoteUpdate>) -> Self {
        Self {
            side,
            tx,
            levels: BTreeMap::new(),
            id_to_price: HashMap::new(),
        }
    }

    pub fn update(&mut self, id: u64, price: f64, size: f64, ts: u64, symbol: &str) {
        if let Some(&old_price) = self.id_to_price.get(&id) {
            self.levels.remove(&OrderedFloat(old_price));
            self.id_to_price.insert(id, price);
        } else {
            self.id_to_price.insert(id, price);
        }

        self.levels.insert(OrderedFloat(price), BookLevel { size, id });

        // _ = ignore the Result — channel full means consumer is behind; backpressure handled by sender spin in engine
        let _ = self.tx.send(QuoteUpdate {
            ts,
            symbol: symbol.to_string(),
            side: self.side,
            price,
            size,
            id,
        });

        if self.levels.len() > BOOK_LEVELS {
            self.prune(symbol);
        }
    }

    pub fn snapshot(&self, symbol: &str, ts: u64) {
        for (price, level) in &self.levels {
            let _ = self.tx.send(QuoteUpdate {
                ts,
                symbol: symbol.to_string(),
                side: self.side,
                price: price.0,
                size: level.size,
                id: level.id,
            });
        }
    }

    fn prune(&mut self, symbol: &str) {
        // Bids: prune lowest (begin). Asks: prune highest (end).
        let key = match self.side {
            Side::Bid => *self.levels.keys().next().unwrap(),
            Side::Ask => *self.levels.keys().next_back().unwrap(),
        };
        let level = self.levels.remove(&key).unwrap();
        self.id_to_price.remove(&level.id);
        let _ = self.tx.send(QuoteUpdate {
            ts: now_micros(),
            symbol: symbol.to_string(),
            side: self.side,
            price: key.0,
            size: 0.0,
            id: level.id,
        });
    }
}
