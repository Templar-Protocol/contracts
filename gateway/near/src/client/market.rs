use std::collections::HashMap;

use blockchain_gateway_core::common::Pagination;
use templar_common::borrow::BorrowPosition;
use templar_common::market::MarketConfiguration;

use crate::client::{macros::contract_views, NearClient};

use super::BoundContractClient;

#[derive(Clone)]
pub struct MarketClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: blockchain_gateway_core::MarketId,
}

impl BoundContractClient for MarketClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id.0
    }
}

impl MarketClient<'_> {
    contract_views! {
        pub fn get_configuration(()) -> MarketConfiguration;
        pub fn list_borrow_positions(Pagination) -> HashMap<near_account_id::AccountId, BorrowPosition>;
    }
}
