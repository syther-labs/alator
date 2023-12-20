use crate::clock::Clock;
use rand::distributions::{Distribution, Uniform};
use rand::thread_rng;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct PenelopeQuote {
    pub bid: f64,
    pub ask: f64,
    pub date: i64,
    pub symbol: String,
}

impl PenelopeQuote {
    pub fn get_bid(&self) -> f64 {
        self.bid
    }

    pub fn get_ask(&self) -> f64 {
        self.ask
    }

    pub fn get_symbol(&self) -> String {
        self.symbol.clone()
    }

    pub fn get_date(&self) -> i64 {
        self.date
    }
}

#[derive(Debug)]
pub struct Penelope {
    inner: HashMap<i64, HashMap<String, PenelopeQuote>>,
}

impl Penelope {
    pub fn get_quote(&self, date: &i64, symbol: &str) -> Option<&PenelopeQuote> {
        if let Some(date_row) = self.inner.get(date) {
            if let Some(quote) = date_row.get(symbol) {
                return Some(quote);
            }
        }
        None
    }

    pub fn get_quotes(&self, date: &i64) -> Option<Vec<PenelopeQuote>> {
        if let Some(date_row) = self.inner.get(date) {
            return Some(date_row.values().cloned().collect());
        }
        None
    }

    pub fn add_quotes(&mut self, bid: f64, ask: f64, date: i64, symbol: impl Into<String> + Clone) {
        let quote = PenelopeQuote {
            bid,
            ask,
            date,
            symbol: symbol.clone().into(),
        };

        if let Some(date_row) = self.inner.get_mut(&date) {
            date_row.insert(symbol.into(), quote);
        } else {
            let mut date_row = HashMap::new();
            date_row.insert(symbol.into(), quote);
            self.inner.insert(date, date_row);
        }
    }

    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn from_hashmap(inner: HashMap<i64, HashMap<String, PenelopeQuote>>) -> Self {
        Self { inner }
    }
}

impl Default for Penelope {
    fn default() -> Self {
        Self::new()
    }
}

/// Generates random [Penelope] for use in tests that don't depend on prices.
pub fn random_penelope_generator(length: i64) -> (Penelope, Clock) {
    let clock = crate::clock::ClockBuilder::with_length_in_seconds(100, length)
        .with_frequency(&crate::clock::Frequency::Second)
        .build();

    let price_dist = Uniform::new(90.0, 100.0);
    let mut rng = thread_rng();

    let mut penelope = Penelope::new();
    for date in clock.peek() {
        penelope.add_quotes(
            price_dist.sample(&mut rng),
            price_dist.sample(&mut rng),
            *date,
            "ABC",
        );
        penelope.add_quotes(
            price_dist.sample(&mut rng),
            price_dist.sample(&mut rng),
            *date,
            "BCD",
        );
    }
    (penelope, clock)
}
