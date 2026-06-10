use crossbeam_channel::Sender;

use crate::book_side::BookSide;
use crate::types::{QuoteUpdate, Side};

pub struct OrderBook {
    sym: String,
    bids: BookSide,
    asks: BookSide,
}

impl OrderBook {
    // tx is Sender<QuoteUpdate> — Clone it so bids and asks each have their own copy.
    // Both clones point to the same channel. No shared_ptr, no reference counting overhead —
    // crossbeam::Sender is an Arc internally.
    pub fn new(symbol: String, tx: Sender<QuoteUpdate>) -> Self {
        let bids = BookSide::new(Side::Bid, tx.clone());
        let asks = BookSide::new(Side::Ask, tx);
        Self { sym: symbol, bids, asks }
    }

    pub fn handle_event(&mut self, side: Side, id: u64, price: f64, size: f64, ts: u64) {
        match side {
            Side::Bid => self.bids.update(id, price, size, ts, &self.sym),
            Side::Ask => self.asks.update(id, price, size, ts, &self.sym),
        }
    }


}
