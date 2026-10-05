//! Escrow dispatch.
//!
//! Exactly one escrow implementation is compiled:
//!
//! - `action-epoch-settlement` selects the share-denominated escrow model
//!   (`epoch_impl`), where settlement must consume escrow exactly and every
//!   payout claim is recomputed from accepted epoch settlement snapshots.
//! - Without the feature, the pre-epoch escrow helpers (`legacy_impl`)
//!   restore the baseline public signatures, layouts, and behavior exactly.

#[cfg(feature = "action-epoch-settlement")]
mod epoch_impl;
#[cfg(feature = "action-epoch-settlement")]
pub use epoch_impl::*;

#[cfg(not(feature = "action-epoch-settlement"))]
mod legacy_impl;
#[cfg(not(feature = "action-epoch-settlement"))]
pub use legacy_impl::*;
