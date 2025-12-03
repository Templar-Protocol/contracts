use alloy::{primitives::U256, sol_types::Eip712Domain};
use near_sdk::{
    json_types::{Base58CryptoHash, U64},
    near, AccountId, CryptoHash,
};

pub const NEAR_MAINNET_CHAIN_ID: u128 = 397;
pub const NEAR_TESTNET_CHAIN_ID: u128 = 398;

pub mod authentication;
pub mod contract_state;
pub mod encoding;
mod event;
pub use event::Event;
mod execute_args;
pub use execute_args::*;
pub mod init_args;
pub use init_args::InitArgs;
pub mod key_id;
pub use key_id::KeyId;
pub mod transaction;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct PayloadExecutionParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
    pub name: Option<String>,
    pub version: Option<String>,
    pub chain_id: Option<near_sdk::json_types::U128>,
    pub verifying_contract: near_sdk::AccountId,
    pub salt: Option<Base58CryptoHash>,
}

impl From<PayloadExecutionParameters> for Eip712Domain {
    fn from(value: PayloadExecutionParameters) -> Self {
        Self {
            name: value.name.map(Into::into),
            version: value.version.map(Into::into),
            chain_id: value.chain_id.map(|i| U256::from(i.0)),
            verifying_contract: Some(
                #[allow(
                    clippy::unwrap_used,
                    reason = "hash len 32 >= 20 && slice len == array len"
                )]
                <[u8; 20]>::try_from(
                    &near_sdk::env::keccak256_array(value.verifying_contract.as_bytes())[0..20],
                )
                .unwrap()
                .into(),
            ),
            salt: value.salt.map(|c| CryptoHash::from(c).into()),
        }
    }
}

impl PayloadExecutionParameters {
    pub fn builder(chain_id: u128) -> PayloadExecutionParametersBuilder<(), (), (), (), ()> {
        PayloadExecutionParametersBuilder::new(chain_id)
    }

    pub fn builder_empty() -> PayloadExecutionParametersBuilder<(), (), (), (), Option<[u8; 32]>> {
        PayloadExecutionParametersBuilder::empty()
    }

    pub fn new_empty(verifying_contract: near_sdk::AccountId) -> Self {
        Self {
            block_height: U64(0),
            index: U64(0),
            nonce: U64(0),
            name: None,
            version: None,
            chain_id: None,
            salt: None,
            verifying_contract,
        }
    }

    #[must_use]
    pub fn next_nonce(self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(deny_unknown_fields)]
pub struct KeyParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
}

impl KeyParameters {
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..*self
        }
    }
}

pub struct PayloadExecutionParametersBuilder<BlockHeight, Index, Nonce, VerifyingContract, Salt> {
    block_height: BlockHeight,
    index: Index,
    nonce: Nonce,
    name: Option<String>,
    version: Option<String>,
    chain_id: Option<u128>,
    verifying_contract: VerifyingContract,
    salt: Salt,
}

macro_rules! builder {
    (@single $field:ident: $t:ty {$($other:ident ,)*}) => {
        #[must_use]
        pub fn $field(self, value: impl Into<$t>) -> Self {
            let Self {
                $($other,)*
            } = self;
            let _ = $field;
            let $field = Some(value.into());
            Self {
                $($other,)*
            }
        }
    };

    (@single $field:ident: $t:ty => <$($g:path),*> {$($other:ident ,)*}) => {
        #[must_use]
        pub fn $field(self, value: impl Into<$t>) -> PayloadExecutionParametersBuilder<$($g),*> {
            let Self {
                $($other,)*
            } = self;
            let _ = $field;
            let $field = value.into();
            PayloadExecutionParametersBuilder {
                $($other,)*
            }
        }
    };

    (
        $( $before:ident : $bt:ty $(=> <$($bg:path),*>)? ,)*
        @mark $field:ident : $t:ty $(=> <$($g:path),*>)? ,
        $($after:ident : $at:ty $(=> <$($ag:path),*>)? ,)*
    ) => {
        builder! { @single $field : $t $(=> <$($g),*>)?
            {
                $($before ,)*
                $field ,
                $($after ,)*
            }
        }

        builder! {
            $( $before : $bt $(=> <$($bg),*>)? ,)*
            $field : $t $(=> <$($g),*>)? ,
            @mark
            $($after : $at $(=> <$($ag),*>)? ,)*
        }
    };

    ( $( $before:ident : $bt:ty $(=> <$($bg:path),*>)? ,)* @mark ) => {};

    ($( $field:ident : $t:ty $(=> <$($g:path),*>)? , )*) => {
        builder! {
            @mark $(
                $field : $t $(=> <$($g),*>)? ,
            )*
        }
    };
}

impl PayloadExecutionParametersBuilder<(), (), (), (), Option<[u8; 32]>> {
    pub fn empty() -> Self {
        Self {
            block_height: (),
            index: (),
            nonce: (),
            name: None,
            version: None,
            chain_id: None,
            verifying_contract: (),
            salt: None,
        }
    }
}

impl PayloadExecutionParametersBuilder<(), (), (), (), ()> {
    pub fn new(chain_id: u128) -> Self {
        Self {
            block_height: (),
            index: (),
            nonce: (),
            name: Some("Templar Universal Account".to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            chain_id: Some(chain_id),
            verifying_contract: (),
            salt: (),
        }
    }
}

impl<B, I, N, V, S> PayloadExecutionParametersBuilder<B, I, N, V, S> {
    builder! {
        block_height: u64 => <u64, I, N, V, S>,
        index: u64 => <B, u64, N, V, S>,
        nonce: u64 => <B, I, u64, V, S>,
        name: String,
        version: String,
        chain_id: u128,
        verifying_contract: AccountId => <B, I, N, AccountId, S>,
        salt: Option<[u8; 32]> => <B, I, N, V, Option<[u8; 32]>>,
    }

    #[must_use]
    pub fn zero(self) -> PayloadExecutionParametersBuilder<u64, u64, u64, V, S> {
        let Self {
            name,
            version,
            chain_id,
            verifying_contract,
            salt,
            ..
        } = self;
        PayloadExecutionParametersBuilder {
            block_height: 0,
            index: 0,
            nonce: 0,
            name,
            version,
            chain_id,
            verifying_contract,
            salt,
        }
    }

    #[must_use]
    pub fn with_key_parameters(
        self,
        key_parameters: KeyParameters,
    ) -> PayloadExecutionParametersBuilder<u64, u64, u64, V, S> {
        let Self {
            name,
            version,
            chain_id,
            verifying_contract,
            salt,
            ..
        } = self;
        PayloadExecutionParametersBuilder {
            block_height: key_parameters.block_height.0,
            index: key_parameters.index.0,
            nonce: key_parameters.nonce.0,
            name,
            version,
            chain_id,
            verifying_contract,
            salt,
        }
    }
}

impl PayloadExecutionParametersBuilder<u64, u64, u64, AccountId, Option<[u8; 32]>> {
    #[must_use]
    pub fn build(self) -> PayloadExecutionParameters {
        PayloadExecutionParameters {
            block_height: self.block_height.into(),
            index: self.index.into(),
            nonce: self.nonce.into(),
            name: self.name,
            version: self.version,
            chain_id: self.chain_id.map(Into::into),
            verifying_contract: self.verifying_contract,
            salt: self.salt.map(Into::into),
        }
    }
}

impl PayloadExecutionParametersBuilder<u64, u64, u64, AccountId, ()> {
    #[must_use]
    pub fn build_salt(self) -> PayloadExecutionParameters {
        let salt = Base58CryptoHash::from(near_sdk::env::keccak256_array(
            #[allow(clippy::unwrap_used, reason = "Infallible")]
            &near_sdk::borsh::to_vec(&(U64(self.block_height), U64(self.index))).unwrap(),
        ));

        PayloadExecutionParameters {
            block_height: self.block_height.into(),
            index: self.index.into(),
            nonce: self.nonce.into(),
            name: self.name,
            version: self.version,
            chain_id: self.chain_id.map(Into::into),
            verifying_contract: self.verifying_contract,
            salt: Some(salt),
        }
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::bs58;
    use p256::elliptic_curve::rand_core::OsRng;
    use solana_sdk::{signature::Keypair, signer::Signer};

    use crate::authentication::{ed25519_raw, passkey::Passkey};

    use super::*;

    #[test]
    fn keyid_serialization() {
        let sk_p256 = p256::SecretKey::random(&mut OsRng);
        let passkey = Passkey(sk_p256.public_key().into());
        let passkey_id: KeyId = passkey.into();
        let passkey_id_str = passkey_id.to_string();

        let Some(b) = passkey_id_str.strip_prefix("p256:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 65);

        let sk_ed25519 = Keypair::new();
        let ed25519_raw = ed25519_raw::VerifyKey(sk_ed25519.pubkey().to_bytes().into());
        let ed25519_raw_id: KeyId = ed25519_raw.into();
        let ed25519_raw_id_str = ed25519_raw_id.to_string();

        let Some(b) = ed25519_raw_id_str.strip_prefix("ed25519:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 32);
    }
}
