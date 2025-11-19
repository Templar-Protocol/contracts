//! Derived from <https://github.com/pyth-network/pyth-crosschain/blob/main/target_chains/near>.
//! Modified for use with the Templar Protocol contracts.
//!
//! The original code was released under the following license:
//!
//! Copyright 2025 Pyth Data Association.
//!
//! Licensed under the Apache License, Version 2.0 (the "License");
//! you may not use this file except in compliance with the License.
//! You may obtain a copy of the License at
//!
//!     http://www.apache.org/licenses/LICENSE-2.0
//!
//! Unless required by applicable law or agreed to in writing, software
//! distributed under the License is distributed on an "AS IS" BASIS,
//! WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//! See the License for the specific language governing permissions and
//! limitations under the License.
use std::{collections::HashMap, fmt::Display};

use near_sdk::{
    ext_contract,
    json_types::{I64, U64},
    near,
};

pub type OracleResponse = HashMap<PriceIdentifier, Option<Price>>;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh, json])]
pub struct PriceIdentifier(
    #[serde(
        serialize_with = "hex::serde::serialize",
        deserialize_with = "hex::serde::deserialize"
    )]
    pub [u8; 32],
);

impl std::fmt::Debug for PriceIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl Display for PriceIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// A price with a degree of uncertainty, represented as a price +- a confidence interval.
///
/// The confidence interval roughly corresponds to the standard error of a normal distribution.
/// Both the price and confidence are stored in a fixed-point numeric representation,
/// `x * (10^expo)`, where `expo` is the exponent.
//
/// Please refer to the documentation at
/// <https://docs.pyth.network/documentation/pythnet-price-feeds/best-practices>
/// for how to use this price safely.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct Price {
    pub price: I64,
    /// Confidence interval around the price
    pub conf: U64,
    /// The exponent
    pub expo: i32,
    /// Unix timestamp of when this price was computed
    pub publish_time: i64,
}

#[ext_contract(ext_pyth)]
pub trait Pyth {
    // See implementations for details, PriceIdentifier can be passed either as a 64 character
    // hex price ID which can be found on the Pyth homepage.
    fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool;
    // fn get_price(&self, price_identifier: PriceIdentifier) -> Option<Price>;
    // fn get_price_unsafe(&self, price_identifier: PriceIdentifier) -> Option<Price>;
    // fn get_price_no_older_than(&self, price_id: PriceIdentifier, age: u64) -> Option<Price>;
    // fn get_ema_price(&self, price_id: PriceIdentifier) -> Option<Price>;
    // fn get_ema_price_unsafe(&self, price_id: PriceIdentifier) -> Option<Price>;
    // fn get_ema_price_no_older_than(&self, price_id: PriceIdentifier, age: u64) -> Option<Price>;
    // fn list_prices(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_prices_unsafe(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_prices_no_older_than(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_ema_prices(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_ema_prices_unsafe(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> HashMap<PriceIdentifier, Option<Price>>;
}
