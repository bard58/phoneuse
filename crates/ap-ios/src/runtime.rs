//! Stack-size helpers for Windows.
//!
//! The idevice futures are large. In a debug build the spike overflowed the
//! 1 MiB Windows main-thread stack before it ran any code. This crate is safe
//! by itself: every device call runs on an internal thread with a
//! [`MAIN_STACK`] stack (see `worker.rs`), and the large futures are boxed
//! there. A caller only needs these helpers for its own large futures.
//!
//! Recommended `main` for a binary that uses ap-ios:
//!
//! ```no_run
//! fn main() -> std::process::ExitCode {
//!     ap_ios::block_on(async {
//!         // ... agentphone code ...
//!         std::process::ExitCode::SUCCESS
//!     })
//! }
//! ```

use std::future::Future;

/// Stack size of the thread that drives the runtime (32 MiB).
pub const MAIN_STACK: usize = 32 * 1024 * 1024;

/// Stack size of each tokio worker thread (8 MiB).
pub const WORKER_STACK: usize = 8 * 1024 * 1024;

/// A multi-thread tokio runtime builder with [`WORKER_STACK`] worker stacks
/// and all drivers enabled. Call `block_on` on a thread with a big stack
/// (or use [`block_on`], which does both).
pub fn runtime_builder() -> tokio::runtime::Builder {
    let mut b = tokio::runtime::Builder::new_multi_thread();
    b.enable_all().thread_stack_size(WORKER_STACK);
    b
}

/// Run `fut` to completion on a new multi-thread runtime, driven from a
/// thread with a [`MAIN_STACK`] stack. The future is boxed, so its state is
/// on the heap. Panics in `fut` resume on the calling thread.
pub fn block_on<F>(fut: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name("agentphone-main".into())
        .stack_size(MAIN_STACK)
        .spawn(move || {
            let rt = runtime_builder()
                .build()
                .expect("cannot build the tokio runtime");
            rt.block_on(Box::pin(fut))
        })
        .expect("cannot spawn the agentphone main thread");
    match handle.join() {
        Ok(v) => v,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn block_on_runs_future() {
        let v = super::block_on(async { 6 * 7 });
        assert_eq!(v, 42);
    }
}
