#![allow(clippy::unwrap_used)]

use near_sdk::serde_json::{self, json};
use templar_common::market::MarketConfiguration;

pub fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let file_path = &args[1];
    let file_contents = std::fs::read(file_path).unwrap();
    let parsed: MarketConfiguration = serde_json::from_slice(&file_contents).unwrap();
    parsed.validate().unwrap();

    println!(
        "{}",
        serde_json::to_string(&json!({
            "configuration": parsed,
        }))
        .unwrap(),
    );
}
