use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use test_utils::{setup_test, worker, ContractController as _};

#[rstest]
#[tokio::test]
#[should_panic = "Duplicate market"]
async fn supply_queue_mustnt_have_duplicates(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
    );
    let m = c.market.contract().id().clone();

    let queue = vec![m.clone(), m.clone()];
    vault.set_supply_queue(&vault_curator, &queue).await;
}

#[rstest]
#[tokio::test]
#[should_panic = "Duplicate market"]
async fn withdraw_queue_mustnt_have_duplicates(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
    );
    let m = c.market.contract().id().clone();

    let queue = vec![m.clone(), m.clone()];
    vault.set_withdraw_queue(&vault_curator, &queue).await;
}

#[rstest]
#[tokio::test]
#[should_panic = "Invariant: Only one op in flight"]
async fn state_machine_is_locked_when_another_op_is_running(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(supply_user, borrow_user)
    );
    let amount = 1000;
    let m = c.market.contract().id().clone();
    vault.supply(&supply_user, amount).await;

    let queue = vec![m.clone()];
    tokio::join!(
        vault.allocate(&vault_curator, vec![], Some(amount.into())),
        vault.submit_cap(&vault_curator, m.clone(), (amount * 2).into()),
        vault.set_supply_queue(&vault_curator, &queue),
        vault.set_withdraw_queue(&vault_curator, &queue),
        vault.allocate(&vault_curator, vec![], Some(amount.into())),
    );
}
