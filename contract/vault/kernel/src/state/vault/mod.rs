//! Vault state dispatch.
//!
//! Exactly one `VaultState` definition is compiled:
//!
//! - `action-epoch-settlement` selects the epoch-settlement vault state
//!   (`epoch_impl`), which carries immutable epoch settlement state inside
//!   `VaultState` and enforces the epoch settlement law in
//!   `check_invariant`.
//! - Without the feature, the baseline vault state (`legacy_impl`) restores
//!   the exact pre-epoch public signatures, layouts, and behavior, with no
//!   epoch field and no epoch invariant checks.

#[cfg(feature = "action-epoch-settlement")]
mod epoch_impl;
#[cfg(feature = "action-epoch-settlement")]
pub use epoch_impl::*;

#[cfg(not(feature = "action-epoch-settlement"))]
mod legacy_impl;
#[cfg(not(feature = "action-epoch-settlement"))]
pub use legacy_impl::*;
