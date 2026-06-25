//! Helper for collecting every page of a paginated gateway read.

use std::future::Future;

/// Repeatedly invoke `fetch_page(offset, page_size)`, collecting every returned
/// item, until a page comes back shorter than `page_size`. The offset advances
/// by the number of items actually returned.
///
/// This is the shared form of the "loop until a short page" pattern that
/// off-chain consumers use to drain a paginated `list_*` read (e.g.
/// `registry.listDeployments`, `market.listBorrowPositions`). It is generic
/// over the fetcher's error type so both `anyhow::Result` and
/// [`GatewayResult`](templar_gateway_core::GatewayResult) callers can use it.
pub async fn collect_paginated<T, E, F, Fut>(page_size: u32, mut fetch_page: F) -> Result<Vec<T>, E>
where
    F: FnMut(u32, u32) -> Fut,
    Fut: Future<Output = Result<Vec<T>, E>>,
{
    let mut all = Vec::new();
    let mut offset = 0_u32;

    loop {
        let page = fetch_page(offset, page_size).await?;
        let fetched = u32::try_from(page.len()).unwrap_or(u32::MAX);
        all.extend(page);

        if fetched < page_size {
            break;
        }
        offset += fetched;
    }

    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// A fetcher over `total` synthetic items returning `page_size`-sized pages,
    /// recording the (offset, count) of every call.
    fn paged_fetcher(
        total: u32,
        calls: &Mutex<Vec<(u32, u32)>>,
    ) -> impl FnMut(u32, u32) -> std::future::Ready<Result<Vec<u32>, Infallible>> + '_ {
        move |offset, count| {
            calls.lock().unwrap().push((offset, count));
            let end = (offset + count).min(total);
            std::future::ready(Ok((offset..end).collect::<Vec<_>>()))
        }
    }

    #[tokio::test]
    async fn stops_on_short_page() {
        let calls = Mutex::new(Vec::new());
        // 250 items, page size 100 -> pages of 100, 100, 50 (short -> stop).
        let items = collect_paginated(100, paged_fetcher(250, &calls))
            .await
            .unwrap();

        assert_eq!(items, (0..250).collect::<Vec<_>>());
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(0, 100), (100, 100), (200, 100)]
        );
    }

    #[tokio::test]
    async fn stops_on_empty_first_page() {
        let calls = Mutex::new(Vec::new());
        let items = collect_paginated(100, paged_fetcher(0, &calls))
            .await
            .unwrap();

        assert!(items.is_empty());
        assert_eq!(*calls.lock().unwrap(), vec![(0, 100)]);
    }

    #[tokio::test]
    async fn makes_extra_call_on_exact_multiple() {
        let calls = Mutex::new(Vec::new());
        // Exactly 200 items: a full second page forces a third (empty) call.
        let items = collect_paginated(100, paged_fetcher(200, &calls))
            .await
            .unwrap();

        assert_eq!(items.len(), 200);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(0, 100), (100, 100), (200, 100)]
        );
    }

    #[tokio::test]
    async fn propagates_errors() {
        let attempts = AtomicU32::new(0);
        let result = collect_paginated(100, |_offset, _count| {
            attempts.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Result::<Vec<u32>, &'static str>::Err("boom"))
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
