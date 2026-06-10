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

#[cfg(test)]
mod tests {
    use super::{buffer, require_at_least};
    use near_sdk::{test_utils::VMContextBuilder, testing_env, Gas};

    #[test]
    fn buffer_adds_twenty_percent_ceiling() {
        assert_eq!(buffer(5), Gas::from_tgas(6));
        assert_eq!(buffer(6), Gas::from_tgas(8));
    }

    #[test]
    fn require_at_least_accepts_sufficient_gas() {
        let mut builder = VMContextBuilder::new();
        builder.prepaid_gas(Gas::from_tgas(10));
        testing_env!(builder.build());

        require_at_least(Gas::from_tgas(8));
    }
}
