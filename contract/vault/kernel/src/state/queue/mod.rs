//! Withdrawal queue dispatch.
//!
//! Exactly one queue implementation is compiled:
//!
//! - `action-epoch-settlement` selects the unpriced, epoch-bound queue
//!   (`epoch_impl`). Requests carry no asset-denominated claim; payouts are
//!   recomputed from accepted epoch settlement snapshots at execution time.
//! - Without the feature, the pre-epoch queue (`legacy_impl`) restores the
//!   baseline public signatures, layouts, and behavior exactly.

#[cfg(feature = "action-epoch-settlement")]
mod epoch_impl;
#[cfg(feature = "action-epoch-settlement")]
pub use epoch_impl::*;

#[cfg(not(feature = "action-epoch-settlement"))]
mod legacy_impl;
#[cfg(not(feature = "action-epoch-settlement"))]
pub use legacy_impl::*;
