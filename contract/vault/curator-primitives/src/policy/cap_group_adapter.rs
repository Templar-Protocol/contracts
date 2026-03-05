//! Shared adapters for cap-group operations from raw field values.

use templar_vault_kernel::Wad;

use super::cap_group::{CapGroup, CapGroupError, CapGroupRecord};

/// Build a [`CapGroup`] from raw absolute/relative cap fields.
#[must_use]
pub fn cap_group_from_fields(absolute_cap: u128, relative_cap: Wad) -> CapGroup {
    CapGroup::new()
        .with_absolute(absolute_cap)
        .with_relative(relative_cap)
}

/// Build a [`CapGroupRecord`] from raw cap fields and principal.
#[must_use]
pub fn cap_group_record_from_fields(
    absolute_cap: u128,
    relative_cap: Wad,
    principal: u128,
) -> CapGroupRecord {
    CapGroupRecord {
        cap: cap_group_from_fields(absolute_cap, relative_cap),
        principal,
    }
}

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
    record.cap = cap_group_from_fields(absolute_cap, relative_cap);
}

/// Update only the relative-cap field of a [`CapGroupRecord`], preserving absolute cap and principal.
pub fn set_cap_group_record_relative_cap(record: &mut CapGroupRecord, relative_cap: Wad) {
    let absolute_cap = cap_group_record_absolute_cap(record);
    record.cap = cap_group_from_fields(absolute_cap, relative_cap);
}

/// Check whether an allocation can be made for raw cap fields.
#[must_use]
pub fn can_allocate_from_fields(
    absolute_cap: u128,
    relative_cap: Wad,
    current_principal: u128,
    amount: u128,
    total_assets: u128,
) -> bool {
    cap_group_from_fields(absolute_cap, relative_cap).can_allocate(
        current_principal,
        amount,
        total_assets,
    )
}

/// Enforce cap constraints for raw cap fields.
pub fn enforce_from_fields(
    absolute_cap: u128,
    relative_cap: Wad,
    current_principal: u128,
    amount: u128,
    total_assets: u128,
) -> Result<(), CapGroupError> {
    cap_group_from_fields(absolute_cap, relative_cap).enforce(
        current_principal,
        amount,
        total_assets,
    )
}

/// Compute effective cap from raw cap fields.
#[must_use]
pub fn effective_cap_from_fields(
    absolute_cap: u128,
    relative_cap: Wad,
    total_assets: u128,
) -> u128 {
    cap_group_from_fields(absolute_cap, relative_cap).effective_cap(total_assets)
}

/// Compute available capacity from raw cap fields.
#[must_use]
pub fn available_capacity_from_fields(
    absolute_cap: u128,
    relative_cap: Wad,
    current_principal: u128,
    total_assets: u128,
) -> u128 {
    cap_group_from_fields(absolute_cap, relative_cap)
        .available_capacity(current_principal, total_assets)
}

#[cfg(test)]
mod tests;
