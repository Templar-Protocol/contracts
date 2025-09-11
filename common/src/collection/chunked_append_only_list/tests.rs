use super::*;

#[test]
fn basic() {
    let mut list = ChunkedAppendOnlyList::<_, 47>::new(b"l");
    assert_eq!(list.len(), 0);
    assert!(list.is_empty());

    for i in 0..10_000usize {
        list.push(i);
        assert_eq!(list.len() as usize, i + 1);
        assert!(!list.is_empty());
    }

    let mut count = 0;
    for (i, v) in list.iter().enumerate() {
        assert_eq!(i, *v);
        count += 1;
    }

    assert_eq!(count, 10_000);
}

#[test]
fn replace_last() {
    let mut list = ChunkedAppendOnlyList::<_, 47>::new(b"l");
    for i in 0..10_000u32 {
        list.push(i);
        list.replace_last(i * 2);
        assert_eq!(list.len(), i + 1);
        assert!(!list.is_empty());
    }

    for i in 0..10_000u32 {
        let x = list.get(i).unwrap();
        assert_eq!(*x, i * 2);
    }

    assert_eq!(list.len(), 10_000);
}

#[test]
fn next_back() {
    let mut list = ChunkedAppendOnlyList::<_, 47>::new(b"l");
    for i in 0..10_000u32 {
        list.push(i);
    }

    let mut it = list.iter();

    let mut i = 10_000;
    while let Some(x) = it.next_back() {
        i -= 1;
        assert_eq!(*x, i);
    }

    assert_eq!(i, 0);
}
