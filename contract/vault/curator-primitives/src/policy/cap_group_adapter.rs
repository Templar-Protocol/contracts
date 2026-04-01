//! Shared adapters for cap-group operations from raw field values.

use templar_vault_kernel::Wad;

use super::cap_group::CapGroupRecord;

/// Read the raw absolute-cap field from a [`CapGroupRecord`].
///
/// Returns `0` when the underlying cap is unset/unlimited.
#[must_use]
pub fn cap_group_record_absolute_cap(record: &CapGroupRecord) -> u128 {
    record.cap.absolute_cap.map(|cap| cap.get()).unwrap_or(0)
}

/// Read the raw relative-cap field from a [`CapGroupRecord`].
///
/// Returns `Wad::one()` when the underlying cap is unset/unlimited.
#[must_use]
pub fn cap_group_record_relative_cap(record: &CapGroupRecord) -> Wad {
    record.cap.relative_cap.unwrap_or(Wad::one())
}

/// Update only the absolute-cap field of a [`CapGroupRecord`], preserving relative cap and principal.
pub fn set_cap_group_record_absolute_cap(record: &mut CapGroupRecord, absolute_cap: u128) {
    let relative_cap = cap_group_record_relative_cap(record);
    record.cap = super::cap_group::CapGroup::builder()
        .absolute_cap(absolute_cap)
        .relative_cap(relative_cap)
        .build();
}

/// Update only the relative-cap field of a [`CapGroupRecord`], preserving absolute cap and principal.
pub fn set_cap_group_record_relative_cap(record: &mut CapGroupRecord, relative_cap: Wad) {
    let absolute_cap = cap_group_record_absolute_cap(record);
    record.cap = super::cap_group::CapGroup::builder()
        .absolute_cap(absolute_cap)
        .relative_cap(relative_cap)
        .build();
}
