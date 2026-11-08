//! Device worker threads.
//!
//! idevice futures are large and some are not `Send`. Each [`IosDevice`]
//! (crate::IosDevice) owns one OS thread with a 32 MiB stack and a
//! current-thread tokio runtime. The backend lives on that thread. Callers
//! send jobs through a channel, so the caller's futures stay small and `Send`
//! and the caller's stack size does not matter.

use std::future::Future;

use ap_core::{Error, Result};
use futures::future::LocalBoxFuture;
use tokio::sync::{mpsc, oneshot};

use crate::backend::IosBackend;
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

/// Handle to a device worker. Dropping it stops the worker after the backend
/// shuts down.
pub(crate) struct Worker {
    tx: mpsc::UnboundedSender<Job>,
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
        Ok((Self { tx }, t))
    }

    /// Run `f` on the worker thread with the backend.
    pub(crate) async fn call<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: for<'a> FnOnce(&'a mut Backend) -> LocalBoxFuture<'a, Result<R>> + Send + 'static,
    {
        let (otx, orx) = oneshot::channel();
        let j = job(move |b| {
            Box::pin(async move {
                let r = f(b).await;
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
    }

    #[async_trait::async_trait(?Send)]
    impl IosBackend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn capabilities(&self) -> ap_core::Capabilities {
            ap_core::Capabilities::NONE
        }
        async fn screenshot(&mut self) -> Result<ap_core::Frame> {
            Ok(ap_core::Frame::solid(2, 2, [0, 0, 0, 255]))
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
        async fn swipe(
            &mut self,
            _: ap_core::Point,
            _: ap_core::Point,
            _: std::time::Duration,
        ) -> Result<()> {
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
    }

    #[tokio::test]
    async fn worker_runs_jobs_in_order() {
        let (w, label) = Worker::start("ap-ios-test".into(), || async {
            let b: Box<Backend> = Box::new(Fake { taps: Vec::new() });
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
}
