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
