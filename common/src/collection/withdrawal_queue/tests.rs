use near_sdk::AccountId;

use super::WithdrawalQueue;

#[test]
fn withdrawal_remove() {
    let mut wq = WithdrawalQueue::new(b"w");

    let alice: AccountId = "alice".parse().unwrap();
    let bob: AccountId = "bob".parse().unwrap();
    let charlie: AccountId = "charlie".parse().unwrap();

    wq.insert_or_update(&alice, 1.into());
    wq.insert_or_update(&bob, 2.into());
    wq.insert_or_update(&charlie, 3.into());
    assert_eq!(wq.len(), 3);
    assert_eq!(wq.remove(&bob), Some(2.into()));
    assert_eq!(wq.len(), 2);
    assert_eq!(wq.remove(&charlie), Some(3.into()));
    assert_eq!(wq.len(), 1);
    assert_eq!(wq.remove(&alice), Some(1.into()));
    assert_eq!(wq.len(), 0);
}

#[test]
fn withdrawal_queueing() {
    let mut wq = WithdrawalQueue::new(b"w");

    let alice: AccountId = "alice".parse().unwrap();
    let bob: AccountId = "bob".parse().unwrap();
    let charlie: AccountId = "charlie".parse().unwrap();

    assert_eq!(wq.len(), 0);
    assert_eq!(wq.peek(), None);
    wq.insert_or_update(&alice, 1.into());
    assert_eq!(wq.len(), 1);
    assert_eq!(wq.peek(), Some((alice.clone(), 1.into())));
    wq.insert_or_update(&alice, 99.into());
    assert_eq!(wq.len(), 1);
    assert_eq!(wq.peek(), Some((alice.clone(), 99.into())));
    wq.insert_or_update(&bob, 123.into());
    assert_eq!(wq.len(), 2);
    wq.try_lock().unwrap();
    assert_eq!(wq.try_pop(), Some((alice.clone(), 99.into())));
    assert_eq!(wq.len(), 1);
    wq.insert_or_update(&charlie, 42.into());
    assert_eq!(wq.len(), 2);
    wq.try_lock().unwrap();
    assert_eq!(wq.try_pop(), Some((bob.clone(), 123.into())));
    assert_eq!(wq.len(), 1);
    wq.try_lock().unwrap();
    assert_eq!(wq.try_pop(), Some((charlie.clone(), 42.into())));
    assert_eq!(wq.len(), 0);
}
