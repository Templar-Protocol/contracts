use near_sdk::near;

use crate::encoding;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct VerifyKey(pub encoding::ethereum::Address);

#[cfg(test)]
mod tests {
    use alloy::{
        primitives::U256,
        signers::{local::PrivateKeySigner, Signer, SignerSync},
        sol_types::SolStruct,
    };

    use crate::{
        authentication::payload::{Payload, SolPayload},
        transaction::{Action, Transaction},
        ExecutionParameters,
    };

    #[test]
    fn sign() {
        let signer = PrivateKeySigner::from_bytes(&[0x55_u8; 32].into())
            .unwrap()
            .with_chain_id(Some(1337));

        // The message to sign.
        let message: SolPayload = Payload {
            parameters: ExecutionParameters::default(),
            account_id: "account_id".parse().unwrap(),
            payload: Box::new([Transaction {
                receiver_id: "receiver".parse().unwrap(),
                actions: Box::new([Action::CreateAccount]),
            }]),
        }
        .try_into()
        .unwrap();

        let domain = alloy::dyn_abi::Eip712Domain {
            name: Some("Templar Universal Account".into()),
            version: Some("0.0.0".into()),
            chain_id: Some(U256::from(397)),
            verifying_contract: Some(alloy::primitives::Address([0x99_u8; 20].into())),
            salt: None,
        };

        // Sign the message asynchronously with the signer.
        let signature = signer.sign_typed_data_sync(&message, &domain).unwrap();

        let signer_address = signer.address();
        println!("Signature produced by {signer_address}: {signature:?}");
        let recovered_address = signature
            .recover_address_from_prehash(&message.eip712_signing_hash(&domain))
            .unwrap();
        println!("Signature recovered address: {recovered_address}");

        assert_eq!(signer_address, recovered_address);
    }
}
