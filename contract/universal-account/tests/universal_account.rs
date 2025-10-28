use near_sdk::{
    serde_json::{self, json},
    NearToken,
};
use near_workspaces::{network::Sandbox, Worker};
use p256::elliptic_curve::rand_core::OsRng;
use rstest::rstest;
use templar_universal_account::{
    authentication::passkey::{
        self,
        data::{AuthenticatorData, ClientDataJson},
        with_raw_string::WithRawString,
        Passkey, Payload, UncheckedMessage,
    },
    encoding::p256::PublicKey,
    transaction::{FunctionCallAction, Transaction},
    ExecuteArgs, KeyId,
};
use test_utils::{
    controller::universal_account::UniversalAccountController, print_execution, worker,
    ContractController, FtController,
};

#[rstest]
#[tokio::test]
pub async fn universal_account(#[future(awt)] worker: Worker<Sandbox>) {
    test_utils::accounts!(worker, uni_account, ft_account, third_party);

    let secret_key = p256::SecretKey::random(&mut OsRng);
    let public_key: PublicKey = secret_key.public_key().into();
    let key_id = KeyId::Passkey(Passkey(public_key.clone()));

    let (uac, ft) = tokio::join!(
        UniversalAccountController::deploy(uni_account, key_id.clone()),
        FtController::deploy(ft_account, "Fungible Token", "FT"),
    );

    let key_list = uac.list_keys(None, None).await;
    assert_eq!(
        key_list,
        vec![key_id.clone()],
        "Key should be the only one in control of the account immediately after deployment"
    );

    let key_entry = uac.get_key(key_id.clone()).await.unwrap();

    assert_eq!(key_entry.index.0, 0);
    assert_eq!(key_entry.nonce.0, 0);

    let payload = WithRawString::from_parsed(Payload {
        parameters: key_entry.next(),
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![
                FunctionCallAction {
                    function_name: "storage_deposit".to_string(),
                    arguments: serde_json::to_vec(&json!({})).unwrap().into(),
                    amount: NearToken::from_near(1).saturating_div(4),
                    gas: near_sdk::Gas::from_tgas(30),
                }
                .into(),
                FunctionCallAction {
                    function_name: "mint".to_string(),
                    arguments: serde_json::to_vec(&json!({
                        "amount": "100",
                    }))
                    .unwrap()
                    .into(),
                    amount: NearToken::from_near(0),
                    gas: near_sdk::Gas::from_tgas(30),
                }
                .into(),
            ]
            .into(),
        }]
        .into(),
    });

    eprintln!("{}", serde_json::to_string_pretty(&payload.parsed).unwrap());

    let challenge = payload.hash();

    let message: passkey::Message<_> = UncheckedMessage::new_and_sign(
        &secret_key,
        payload,
        AuthenticatorData(Box::new([0xff_u8; 32])),
        WithRawString::from_parsed(ClientDataJson {
            r#type: "type".to_string(),
            challenge: challenge.into(),
            origin: "origin".to_string(),
            cross_origin: None,
            top_origin: None,
        }),
    )
    .try_into()
    .unwrap();

    eprintln!("{}", serde_json::to_string_pretty(&message).unwrap());

    let e = uac
        .execute(
            &third_party,
            ExecuteArgs::Passkey {
                key: Passkey(public_key.clone()),
                message,
            },
        )
        .await;

    print_execution(&e);

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");
}
