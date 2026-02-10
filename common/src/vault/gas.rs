use super::*;

// Add a 20% buffer to a gas estimate
#[must_use]
pub const fn buffer(size: u64) -> Gas {
    Gas::from_tgas((size * 6).div_ceil(5))
}

pub fn require_at_least(needed: Gas) {
    let gas = env::prepaid_gas();
    require!(
        gas >= needed,
        format!("Insufficient gas: {}, needed: {needed}", gas)
    );
}
