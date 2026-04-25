pub mod governance;
pub mod input;
pub mod price_transformer;
pub mod request;
pub mod state;

use near_sdk::json_types::{I64, U64};
use templar_common::oracle::pyth::{self, PythTimestamp};

pub fn pyth_to_kernel(p: &pyth::Price) -> Option<templar_proxy_oracle_kernel::Price> {
    Some(templar_proxy_oracle_kernel::Price {
        price: p.price.0,
        conf: p.conf.0,
        expo: p.expo,
        publish_time_ns: p.publish_time.try_into_time()?,
    })
}

pub fn kernel_to_pyth(p: &templar_proxy_oracle_kernel::Price) -> Option<pyth::Price> {
    Some(pyth::Price {
        price: I64(p.price),
        conf: U64(p.conf),
        expo: p.expo,
        publish_time: PythTimestamp::try_from_time(p.publish_time_ns)?,
    })
}
