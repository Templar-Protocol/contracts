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
#[should_panic = "Invariant: Only one op in flight"]
async fn state_machine_is_locked_when_another_op_is_running(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    setup_test!(
        worker
        extract(vault, c, vault_owner)
        accounts(supply_user, borrow_user)
    );
    let amount = 1000;
    vault.supply(&supply_user, amount).await;

    futures::future::select_all(
        (0..100).map(|_| Box::pin(vault.allocate(&vault_owner, vec![], Some(1.into())))),
    )
    .await;
}
