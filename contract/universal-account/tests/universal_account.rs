use near_sdk::{
    json_types::U64,
    serde_json::{self, json},
    NearToken,
};
use p256::elliptic_curve::rand_core::OsRng;
use templar_universal_account::{
    authentication::passkey::{
        data::{AuthenticatorData, ClientDataJson},
        with_raw_string::WithRawString,
        Message, Passkey, Payload,
    },
    encoding::p256::PublicKey,
    transaction::{Action, Transaction},
    ExecutionParameters, KeyId,
};
use test_utils::{
    controller::universal_account::UniversalAccountController, print_execution, ContractController,
    FtController,
};

#[tokio::test]
pub async fn universal_account() {
    let worker = near_workspaces::sandbox().await.unwrap();

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

    assert_eq!(
        key_entry,
        ExecutionParameters {
            index: U64(0),
            nonce: U64(0),
        },
        "Nonce and index should be zero immediately after deployment",
    );

    let payload = WithRawString::from_parsed(Payload {
        parameters: ExecutionParameters {
            index: U64(0),
            nonce: U64(1),
        },
        account_id: uac.contract().id().clone(),
        payload: vec![Transaction {
            receiver_id: ft.contract().id().clone(),
            actions: vec![
                Action::FunctionCall {
                    function_name: "storage_deposit".to_string(),
                    arguments: serde_json::to_vec(&json!({})).unwrap().into(),
                    amount: NearToken::from_near(1).saturating_div(4),
                    gas: near_sdk::Gas::from_tgas(30),
                },
                Action::FunctionCall {
                    function_name: "mint".to_string(),
                    arguments: serde_json::to_vec(&json!({
                        "amount": "100",
                    }))
                    .unwrap()
                    .into(),
                    amount: NearToken::from_near(0),
                    gas: near_sdk::Gas::from_tgas(30),
                },
            ],
        }],
    });

    eprintln!("{}", serde_json::to_string_pretty(&payload.parsed).unwrap());

    let challenge = payload.hash();

    let message = Message::new_and_sign(
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
    );

    // eprintln!("{message:#?}");
    eprintln!("{}", serde_json::to_string_pretty(&message).unwrap());

    let e = uac
        .execute_passkey(&third_party, key_id.clone(), message)
        .await;

    print_execution(&e);

    let balance = ft.ft_balance_of(uac.contract.id()).await;
    assert_eq!(balance.0, 100, "Function call should succeed");
}
