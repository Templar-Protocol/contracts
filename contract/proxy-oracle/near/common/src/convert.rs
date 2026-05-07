use near_sdk::json_types::{I64, U64};
use near_sdk::AccountIdRef;
use templar_common::oracle::pyth::{self, PythTimestamp};

pub fn pyth_price_try_to_kernel(p: &pyth::Price) -> Option<templar_proxy_oracle_kernel::Price> {
    Some(templar_proxy_oracle_kernel::Price {
        price: p.price.0,
        conf: p.conf.0,
        expo: p.expo,
        publish_time_ns: p.publish_time.try_into_time()?,
    })
}

pub fn account_id_to_kernel(
    account_id: &AccountIdRef,
) -> templar_proxy_oracle_kernel::primitive::AccountId {
    let mut bytes = [0u8; 64];
    let source = account_id.as_bytes();
    bytes[..source.len()].copy_from_slice(source);
    templar_proxy_oracle_kernel::primitive::AccountId::from_bytes(bytes)
}

pub fn pyth_price_try_from_kernel(p: &templar_proxy_oracle_kernel::Price) -> Option<pyth::Price> {
    Some(pyth::Price {
        price: I64(p.price),
        conf: U64(p.conf),
        expo: p.expo,
        publish_time: PythTimestamp::try_from_time(p.publish_time_ns)?,
    })
}

#[cfg(test)]
mod tests {
    use super::account_id_to_kernel;
    use near_sdk::AccountId;

    #[test]
    fn converts_near_account_id_to_zero_padded_kernel_bytes() {
        let account_id: AccountId = "oracle.near".parse().unwrap();
        let converted = account_id_to_kernel(account_id.as_ref());

        assert_eq!(
            &converted.as_bytes()[..account_id.len()],
            account_id.as_bytes()
        );
        assert!(converted.as_bytes()[account_id.len()..]
            .iter()
            .all(|byte| *byte == 0));
    }

    #[test]
    fn converts_max_length_near_account_id_to_kernel_bytes() {
        let account_id: AccountId = "a".repeat(64).parse().unwrap();
        let converted = account_id_to_kernel(account_id.as_ref());

        assert_eq!(converted.as_bytes(), account_id.as_bytes());
    }
}
