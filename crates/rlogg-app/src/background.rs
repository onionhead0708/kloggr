//! Long-running blocking jobs (indexing, search).

use gpui::Context;

/// Runs a blocking job off the UI thread. It gets its own thread rather than
/// a GPUI background task, so a long indexing run or search never occupies
/// GPUI's executor. GPUI's test scheduler rejects wake-ups from foreign
/// threads, so tests run the job as a (deterministic) background task.
pub fn spawn_blocking<T>(name: &str, job: impl FnOnce() + Send + 'static, cx: &mut Context<T>) {
    #[cfg(not(test))]
    {
        let _ = cx;
        std::thread::Builder::new()
            .name(name.into())
            .spawn(job)
            .expect("failed to spawn a worker thread");
    }
    #[cfg(test)]
    {
        let _ = name;
        cx.background_executor()
            .spawn(async move { job() })
            .detach();
    }
}

/// Runs a blocking job like [`spawn_blocking`] and returns its result,
/// for use from an async task. `None` if the job panicked.
pub async fn run_blocking<R: Send + 'static>(
    name: &str,
    executor: &gpui::BackgroundExecutor,
    job: impl FnOnce() -> R + Send + 'static,
) -> Option<R> {
    let (tx, rx) = async_channel::bounded(1);
    let job = move || {
        tx.send_blocking(job()).ok();
    };
    #[cfg(not(test))]
    {
        let _ = executor;
        std::thread::Builder::new()
            .name(name.into())
            .spawn(job)
            .expect("failed to spawn a worker thread");
    }
    #[cfg(test)]
    {
        let _ = name;
        executor.spawn(async move { job() }).detach();
    }
    rx.recv().await.ok()
}
