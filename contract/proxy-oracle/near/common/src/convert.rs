use near_sdk::json_types::{I64, U64};
use near_sdk::{AccountId, AccountIdRef};
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

pub fn account_id_try_from_kernel(
    account_id: templar_proxy_oracle_kernel::primitive::AccountId,
) -> Option<AccountId> {
    let bytes = account_id.as_bytes();
    let len = if let Some(len) = bytes.iter().position(|byte| *byte == 0) {
        if bytes[len..].iter().any(|byte| *byte != 0) {
            return None;
        }
        len
    } else {
        bytes.len()
    };
    let account_id = core::str::from_utf8(&bytes[..len]).ok()?;
    account_id.parse().ok()
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
    use super::{account_id_to_kernel, account_id_try_from_kernel};
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

    #[test]
    fn converts_kernel_account_id_to_near_account_id() {
        let account_id: AccountId = "oracle.near".parse().unwrap();
        let converted = account_id_to_kernel(account_id.as_ref());

        assert_eq!(account_id_try_from_kernel(converted), Some(account_id));
    }

    #[test]
    fn rejects_kernel_account_id_with_non_zero_bytes_after_padding() {
        let account_id: AccountId = "oracle.near".parse().unwrap();
        let mut bytes = *account_id_to_kernel(account_id.as_ref()).as_bytes();
        bytes[account_id.len() + 1] = b'x';
        let converted = templar_proxy_oracle_kernel::primitive::AccountId::from_bytes(bytes);

        assert_eq!(account_id_try_from_kernel(converted), None);
    }
}
