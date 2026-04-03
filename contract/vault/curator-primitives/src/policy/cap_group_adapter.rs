//! Shared adapters for cap-group operations from raw field values.

use templar_vault_kernel::Wad;

use super::cap_group::CapGroupRecord;

/// Read the raw absolute-cap field from a [`CapGroupRecord`].
///
/// Returns `0` when the underlying cap is unset/unlimited.
#[must_use]
pub fn cap_group_record_absolute_cap(record: &CapGroupRecord) -> u128 {
    record
        .cap
        .absolute_cap()
        .map_or(0, core::num::NonZeroU128::get)
}

/// Read the raw relative-cap field from a [`CapGroupRecord`].
///
/// Returns `Wad::one()` when the underlying cap is unset/unlimited.
#[must_use]
pub fn cap_group_record_relative_cap(record: &CapGroupRecord) -> Wad {
    record.cap.relative_cap().unwrap_or(Wad::one())
}

/// Update only the absolute-cap field of a [`CapGroupRecord`], preserving relative cap and principal.
pub fn set_cap_group_record_absolute_cap(record: &mut CapGroupRecord, absolute_cap: u128) {
    record.cap.set_absolute_cap(absolute_cap);
}

/// Update only the relative-cap field of a [`CapGroupRecord`], preserving absolute cap and principal.
pub fn set_cap_group_record_relative_cap(record: &mut CapGroupRecord, relative_cap: Wad) {
    record.cap.set_relative_cap(relative_cap);
}
