use std::collections::HashMap;

use crossbeam_channel::Sender;

use crate::order_book::OrderBook;
use crate::time::now_micros;
use crate::types::{Config, MarketEvent, QuoteUpdate, Side, Symbol, BOOK_LEVELS};

pub struct BooksManager {
    books: HashMap<Symbol, OrderBook>,
}

impl BooksManager {
    pub fn new() -> Self {
        Self { books: HashMap::new() }
    }

    pub fn populate_and_init_books(&mut self, config: &Config, tx: Sender<QuoteUpdate>) {
        let ts = now_micros();
        for &sym in &config.symbols {
            // tx.clone() — each OrderBook gets its own Sender clone (same channel, multiple producers ok)
            let book = OrderBook::new(sym, tx.clone());
            self.books.insert(sym, book);

            let mid = config.initial_mid_prices[&sym];
            let spread = mid * 0.0005;

            for i in 0..BOOK_LEVELS as u64 {
                let bid_price = mid - spread * (i + 1) as f64;
                let ask_price = mid + spread * (i + 1) as f64;
                let book = self.books.get_mut(&sym).unwrap();
                book.handle_event(Side::Bid, i, bid_price, (i + 1) as f64, ts);
                book.handle_event(Side::Ask, i, ask_price, (i + 1) as f64, ts);
            }
        }
    }

    pub fn process_event(&mut self, event: &MarketEvent) {
        if let Some(book) = self.books.get_mut(&event.symbol) {
            book.handle_event(event.side, event.id, event.price, event.size, event.ts);
        } else {
            eprintln!("unknown symbol: {}", event.symbol.as_str());
        }
    }
}
