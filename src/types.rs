use std::collections::HashMap;

pub const BOOK_LEVELS: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Bid => "bid",
            Side::Ask => "ask",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RateProfile {
    Constant,
    SineWave,
    Burst,
    Ramp,
}

// size is not used in this scenario - but is required
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct BookLevel {
    pub size: f64,
    pub id: u64,
}

#[derive(Clone, Debug)]
pub struct MarketEvent {
    pub ts: u64,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub size: f64,
    pub id: u64,
}

#[derive(Clone, Debug)]
pub struct QuoteUpdate {
    pub ts: u64,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub size: f64,
    pub id: u64,
}

#[derive(Clone)]
pub struct Config {
    pub rate_profile: RateProfile,
    pub base_rate: f64,
    pub max_rate: f64,
    pub data_mode: String,
    pub symbols: Vec<String>,
    pub initial_mid_prices: HashMap<String, f64>,
}
