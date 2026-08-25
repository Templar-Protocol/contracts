use templar_proxy_oracle_kernel::proxy::{Aggregator, Proxy};

use crate::input::Source;

/// A zero-weight source is excluded from the weighted median and from its quorum count, so a proxy
/// carrying one advertises more redundancy than it has.
#[must_use]
pub fn has_zero_weighted_source(proxy: &Proxy<Source>) -> bool {
    match &proxy.aggregator {
        Aggregator::MedianLow(median) => median.sources.iter().any(|source| source.weight == 0),
        Aggregator::MedianHigh(median) => median.sources.iter().any(|source| source.weight == 0),
        Aggregator::Priority(_) => false,
    }
}
