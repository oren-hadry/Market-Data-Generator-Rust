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

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    const SYM: &str = "BTCUSD";
    const TS: u64 = 1000;

    fn make_book(side: Side) -> (BookSide, crossbeam_channel::Receiver<QuoteUpdate>) {
        let (tx, rx) = unbounded();
        (BookSide::new(side, tx), rx)
    }

    fn drain(rx: &crossbeam_channel::Receiver<QuoteUpdate>) -> Vec<QuoteUpdate> {
        let mut v = vec![];
        while let Ok(q) = rx.try_recv() {
            v.push(q);
        }
        v
    }

    #[test]
    fn new_id_adds_level() {
        let (mut book, rx) = make_book(Side::Bid);
        book.update(1, 100.0, 2.0, TS, SYM);

        let updates = drain(&rx);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].price, 100.0);
        assert_eq!(updates[0].size, 2.0);
        assert_eq!(updates[0].id, 1);
    }

    #[test]
    fn same_id_same_price_updates_size() {
        let (mut book, rx) = make_book(Side::Bid);
        book.update(1, 100.0, 2.0, TS, SYM);
        book.update(1, 100.0, 5.0, TS, SYM);

        let updates = drain(&rx);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].price, 100.0);
        assert_eq!(updates[1].size, 5.0);
        assert_eq!(updates[1].id, 1);
    }

    #[test]
    fn same_id_new_price_moves_level() {
        let (mut book, rx) = make_book(Side::Bid);
        book.update(1, 100.0, 2.0, TS, SYM);
        book.update(1, 200.0, 3.0, TS, SYM);

        // only 2 updates — old level removed silently from BTreeMap, new price emitted
        let updates = drain(&rx);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].price, 200.0);
        assert_eq!(updates[1].size, 3.0);
    }

    #[test]
    fn bid_prunes_lowest_price() {
        let (mut book, rx) = make_book(Side::Bid);
        for i in 1u64..=101 {
            book.update(i, i as f64, 1.0, TS, SYM);
        }

        let updates = drain(&rx);
        let prune = updates.last().unwrap();
        assert_eq!(prune.price, 1.0);
        assert_eq!(prune.size, 0.0);
    }

    #[test]
    fn ask_prunes_highest_price() {
        let (mut book, rx) = make_book(Side::Ask);
        for i in 1u64..=101 {
            book.update(i, i as f64, 1.0, TS, SYM);
        }

        let updates = drain(&rx);
        let prune = updates.last().unwrap();
        assert_eq!(prune.price, 101.0);
        assert_eq!(prune.size, 0.0);
    }

    #[test]
    fn never_exceeds_max_levels() {
        let (mut book, rx) = make_book(Side::Bid);
        for i in 1u64..=200 {
            book.update(i, i as f64, 1.0, TS, SYM);
        }

        // Count net live levels: +1 for size>0 (insert), -1 for size=0 (prune).
        // Check final count, matching C++ test semantics — the prune fires within
        // the same update call, so the net after each update stays <= BOOK_LEVELS.
        let updates = drain(&rx);
        let mut live: i64 = 0;
        for q in &updates {
            if q.size > 0.0 { live += 1; } else { live -= 1; }
        }
        assert!(live <= BOOK_LEVELS as i64);
    }

    #[test]
    fn prune_emits_correct_id() {
        let (mut book, rx) = make_book(Side::Bid);
        // id=200 at price=1.0 — worst bid (lowest price), will be pruned first
        book.update(200, 1.0, 1.0, TS, SYM);
        for i in 1u64..=100 {
            book.update(i, (i + 1) as f64, 1.0, TS, SYM);
        }

        let updates = drain(&rx);
        let prune = updates.last().unwrap();
        assert_eq!(prune.id, 200);
        assert_eq!(prune.size, 0.0);
    }
}
