//! Device worker threads.
//!
//! idevice futures are large and some are not `Send`. Each [`IosDevice`]
//! (crate::IosDevice) owns one OS thread with a 32 MiB stack and a
//! current-thread tokio runtime. The backend lives on that thread. Callers
//! send jobs through a channel, so the caller's futures stay small and `Send`
//! and the caller's stack size does not matter.
//!
//! Each job has a deadline that the worker thread enforces. When it expires,
//! the worker drops the job future, tells the backend to drop its session
//! ([`IosBackend::abandon`]), and returns [`Error::Timeout`]. So one stalled
//! read cannot block the worker and every later call for good. Backends set
//! tighter deadlines of their own. This one is the backstop.

use std::future::Future;
use std::time::Duration;

use ap_core::{Error, Result};
use futures::future::LocalBoxFuture;
use tokio::sync::{mpsc, oneshot};

use crate::backend::IosBackend;
use crate::cancel::{CancelHandle, cancelled};
use crate::runtime::MAIN_STACK;

type Backend = dyn IosBackend + 'static;
type Job = Box<dyn for<'a> FnOnce(&'a mut Backend) -> LocalBoxFuture<'a, ()> + Send>;

fn job<F>(f: F) -> Job
where
    F: for<'a> FnOnce(&'a mut Backend) -> LocalBoxFuture<'a, ()> + Send + 'static,
{
    Box::new(f)
}

fn stopped() -> Error {
    Error::NotConnected("the iOS device worker has stopped".into())
}

/// Build a current-thread runtime on a fresh big-stack thread and run `f`.
fn spawn_thread<F>(name: String, f: F) -> Result<()>
where
    F: FnOnce(tokio::runtime::Runtime) + Send + 'static,
{
    std::thread::Builder::new()
        .name(name)
        .stack_size(MAIN_STACK)
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => f(rt),
                Err(e) => tracing::error!("cannot build the iOS worker runtime: {e}"),
            }
        })
        .map(|_| ())
        .map_err(Error::Io)
}

/// Run one job on a fresh big-stack thread and wait for its result. For
/// discovery and setup steps that do not keep a session.
pub(crate) async fn offload<T, F, Fut>(f: F) -> Result<T>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T>> + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    spawn_thread("ap-ios-task".into(), move |rt| {
        let local = tokio::task::LocalSet::new();
        let r = local.block_on(&rt, Box::pin(f()));
        let _ = tx.send(r);
    })?;
    rx.await.map_err(|_| stopped())?
}

/// Default deadline of one job in tests (see `Worker::call`).
#[cfg(test)]
const JOB_DEADLINE: Duration = Duration::from_secs(120);

/// Handle to a device worker. Dropping it stops the worker after the backend
/// shuts down.
pub(crate) struct Worker {
    tx: mpsc::UnboundedSender<Job>,
    cancel: CancelHandle,
}

impl Worker {
    /// Start a worker. `init` runs on the worker thread and builds the
    /// backend. Its extra value `T` is returned to the caller.
    pub(crate) async fn start<T, F, Fut>(name: String, init: F) -> Result<(Self, T)>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(Box<Backend>, T)>> + 'static,
        T: Send + 'static,
    {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let (init_tx, init_rx) = oneshot::channel::<Result<T>>();
        spawn_thread(name, move |rt| {
            let local = tokio::task::LocalSet::new();
            local.block_on(
                &rt,
                Box::pin(async move {
                    let mut backend = match Box::pin(init()).await {
                        Ok((b, t)) => {
                            if init_tx.send(Ok(t)).is_err() {
                                return;
                            }
                            b
                        }
                        Err(e) => {
                            let _ = init_tx.send(Err(e));
                            return;
                        }
                    };
                    while let Some(j) = rx.recv().await {
                        j(&mut *backend).await;
                    }
                    backend.shutdown().await;
                }),
            );
        })?;
        let t = init_rx.await.map_err(|_| stopped())??;
        Ok((
            Self {
                tx,
                cancel: CancelHandle::default(),
            },
            t,
        ))
    }

    /// A handle that cancels the jobs submitted before its `cancel` call.
    pub(crate) fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Run `f` on the worker thread with the backend, with the default
    /// deadline `JOB_DEADLINE`. Device code uses [`Worker::call_within`]
    /// with a deadline per operation.
    #[cfg(test)]
    pub(crate) async fn call<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: for<'a> FnOnce(&'a mut Backend) -> LocalBoxFuture<'a, Result<R>> + Send + 'static,
    {
        self.call_within(JOB_DEADLINE, f).await
    }

    /// Run `f` on the worker thread with the backend. The worker stops the
    /// job after `deadline` (counted from the start of the job, not from the
    /// submit) and returns [`Error::Timeout`]. A job cancelled before it
    /// starts does not run.
    pub(crate) async fn call_within<R, F>(&self, deadline: Duration, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: for<'a> FnOnce(&'a mut Backend) -> LocalBoxFuture<'a, Result<R>> + Send + 'static,
    {
        let check = self.cancel.check();
        let (otx, orx) = oneshot::channel();
        let j = job(move |b| {
            Box::pin(async move {
                if check.is_set() {
                    let _ = otx.send(Err(cancelled()));
                    return;
                }
                b.set_cancel(check);
                let r = match tokio::time::timeout(deadline, f(&mut *b)).await {
                    Ok(r) => r,
                    Err(_) => {
                        tracing::warn!("iOS call stopped after {deadline:?}; dropping the session");
                        b.abandon();
                        Err(Error::Timeout(deadline))
                    }
                };
                let _ = otx.send(r);
            })
        });
        self.tx.send(j).map_err(|_| stopped())?;
        orx.await.map_err(|_| stopped())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn offload_runs_on_big_stack() {
        let name = offload(|| async {
            Ok(std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string())
        })
        .await
        .unwrap();
        assert_eq!(name, "ap-ios-task");
        let e = offload(|| async { Err::<(), _>(Error::Other("x".into())) })
            .await
            .unwrap_err();
        assert!(matches!(e, Error::Other(_)));
    }

    struct Fake {
        taps: Vec<ap_core::Point>,
        cancel: crate::cancel::CancelCheck,
        abandoned: bool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                taps: Vec::new(),
                cancel: Default::default(),
                abandoned: false,
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl IosBackend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn capabilities(&self) -> ap_core::Capabilities {
            ap_core::Capabilities::NONE
        }
        /// 2x2, or 3x3 after `abandon`.
        async fn screenshot(&mut self) -> Result<ap_core::Frame> {
            let n = if self.abandoned { 3 } else { 2 };
            Ok(ap_core::Frame::solid(n, n, [0, 0, 0, 255]))
        }
        async fn tap(&mut self, at: ap_core::Point) -> Result<()> {
            self.taps.push(at);
            Ok(())
        }
        async fn double_tap(&mut self, _: ap_core::Point) -> Result<()> {
            Ok(())
        }
        async fn long_press(&mut self, _: ap_core::Point, _: std::time::Duration) -> Result<()> {
            Ok(())
        }
        /// Stands in for a long drag: 100 points, 10 ms apart, with a
        /// cancel check between points.
        async fn swipe(
            &mut self,
            _: ap_core::Point,
            _: ap_core::Point,
            _: std::time::Duration,
        ) -> Result<()> {
            for _ in 0..100 {
                if self.cancel.is_set() {
                    return Err(cancelled());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(())
        }
        async fn type_text(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn key(&mut self, _: &ap_core::Key) -> Result<()> {
            Ok(())
        }
        async fn button(&mut self, _: ap_core::Button) -> Result<()> {
            Ok(())
        }
        async fn launch_app(&mut self, _: &str) -> Result<()> {
            Err(Error::unsupported("launch_app"))
        }
        /// A read that never completes.
        async fn list_apps(&mut self) -> Result<Vec<String>> {
            std::future::pending().await
        }
        fn set_cancel(&mut self, c: crate::cancel::CancelCheck) {
            self.cancel = c;
        }
        fn abandon(&mut self) {
            self.abandoned = true;
        }
    }

    #[tokio::test]
    async fn worker_runs_jobs_in_order() {
        let (w, label) = Worker::start("ap-ios-test".into(), || async {
            let b: Box<Backend> = Box::new(Fake::new());
            Ok((b, "ready"))
        })
        .await
        .unwrap();
        assert_eq!(label, "ready");
        w.call(|b| b.tap(ap_core::Point::CENTER)).await.unwrap();
        w.call(|b| b.tap(ap_core::Point::new(0.1, 0.2)))
            .await
            .unwrap();
        let f = w.call(|b| b.screenshot()).await.unwrap();
        assert_eq!((f.width, f.height), (2, 2));
        assert!(
            w.call(|b| b.launch_app("x"))
                .await
                .unwrap_err()
                .is_unsupported()
        );
        let e = Worker::start("ap-ios-test2".into(), || async {
            Err::<(Box<Backend>, ()), _>(Error::setup("p", "h"))
        })
        .await
        .err()
        .unwrap();
        assert_eq!(e.hint(), Some("h"));
    }

    async fn start_fake() -> Worker {
        Worker::start("ap-ios-test3".into(), || async {
            let b: Box<Backend> = Box::new(Fake::new());
            Ok((b, ()))
        })
        .await
        .unwrap()
        .0
    }

    #[tokio::test]
    async fn stalled_job_times_out_and_worker_keeps_going() {
        let w = start_fake().await;
        let e = w
            .call_within(Duration::from_millis(50), |b| b.list_apps())
            .await
            .unwrap_err();
        assert!(matches!(e, Error::Timeout(_)), "{e}");
        // The worker is free again, and the backend was told to drop its session.
        let f = w.call(|b| b.screenshot()).await.unwrap();
        assert_eq!(f.width, 3);
    }

    #[tokio::test]
    async fn cancel_stops_running_and_queued_jobs() {
        let w = start_fake().await;
        let cancel = w.cancel_handle();
        let p = ap_core::Point::CENTER;
        let long = w.call(move |b| b.swipe(p, p, Duration::from_secs(1)));
        let queued = w.call(move |b| b.tap(p));
        let stop = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        };
        let (long, queued, ()) = tokio::join!(long, queued, stop);
        assert!(crate::cancel::is_cancelled(&long.unwrap_err()));
        assert!(crate::cancel::is_cancelled(&queued.unwrap_err()));
        // A call after the cancel runs normally.
        w.call(move |b| b.tap(p)).await.unwrap();
    }
}
