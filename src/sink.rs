use crate::types::QuoteUpdate;

// trait = pure virtual class in C++. `Send` required: sinks run in the consumer thread.
pub trait Sink: Send {
    fn consume(&mut self, q: &QuoteUpdate);
}

pub struct StdoutSink;

impl Sink for StdoutSink {
    fn consume(&mut self, q: &QuoteUpdate) {
        // println! goes to stdout. eprintln! goes to stderr (used for logs).
        println!(
            "{},{},{},{:.5},{:.4},{}",
            q.ts,
            q.symbol.as_str(),
            q.side.as_str(),
            q.price,
            q.size,
            q.id,
        );
    }
}
