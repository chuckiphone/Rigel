//! Ordered execution for independent, read-only agent tools.

use std::future::Future;

/// Run a group concurrently while retaining the input order in the returned results.
pub async fn run_ordered<F, T>(tasks: impl IntoIterator<Item = F>) -> Vec<T>
where
    F: Future<Output = T>,
{
    futures::future::join_all(tasks).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::run_ordered;

    #[tokio::test]
    async fn concurrent_tasks_finish_in_any_order_but_return_in_request_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(Mutex::new(Vec::new()));
        let tasks = [
            (1, 30), // The first request is deliberately the slowest.
            (2, 1),
            (3, 10),
        ]
        .into_iter()
        .map(|(value, delay)| {
            let active = active.clone();
            let peak = peak.clone();
            let completed = completed.clone();
            async move {
                let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now_active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(delay)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                completed.lock().unwrap().push(value);
                value * 10
            }
        });

        let results = run_ordered(tasks).await;
        assert_eq!(results, vec![10, 20, 30]);
        assert!(peak.load(Ordering::SeqCst) > 1, "tasks overlapped");
        assert_eq!(*completed.lock().unwrap(), vec![2, 3, 1]);
    }
}
