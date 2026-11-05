//! A one-shot reply from the writer thread that works without an async
//! runtime: await it from async code, or `wait` for it from a plain thread.
//!
//! Written here, on `parking_lot`, rather than taken from a crate: the store
//! must not depend on tokio, `futures-channel` has no blocking receive, and
//! this is a mutex, a condvar, and a waker.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use parking_lot::{Condvar, Mutex};

use crate::StoreError;

struct Slot<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

struct State<T> {
    value: Option<T>,
    /// The sender sent or was dropped: no value is coming after this.
    closed: bool,
    waker: Option<Waker>,
}

/// The writer's end: sends the value once. Dropping it unsent tells the
/// receiver no value is coming.
pub(crate) struct Sender<T> {
    slot: Arc<Slot<T>>,
}

/// The caller's end of a one-shot reply.
pub(crate) struct Receiver<T> {
    slot: Arc<Slot<T>>,
}

/// A new reply channel.
pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let slot = Arc::new(Slot {
        state: Mutex::new(State {
            value: None,
            closed: false,
            waker: None,
        }),
        ready: Condvar::new(),
    });
    (
        Sender {
            slot: Arc::clone(&slot),
        },
        Receiver { slot },
    )
}

impl<T> Sender<T> {
    /// Sends the value. The receiver may already be gone; then it is dropped.
    pub(crate) fn send(self, value: T) {
        self.slot.state.lock().value = Some(value);
        // `Drop` closes the slot and wakes the receiver.
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.slot.state.lock();
            state.closed = true;
            state.waker.take()
        };
        // Outside the lock: a waker is code we don't own.
        self.slot.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Receiver<T> {
    /// Blocks the thread until the value arrives; `None` if it never will.
    pub(crate) fn wait(self) -> Option<T> {
        let mut state = self.slot.state.lock();
        loop {
            if let Some(value) = state.value.take() {
                return Some(value);
            }
            if state.closed {
                return None;
            }
            self.slot.ready.wait(&mut state);
        }
    }

    fn poll_value(&self, cx: &Context<'_>) -> Poll<Option<T>> {
        let mut state = self.slot.state.lock();
        if let Some(value) = state.value.take() {
            return Poll::Ready(Some(value));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        match &mut state.waker {
            Some(waker) if waker.will_wake(cx.waker()) => {}
            slot => *slot = Some(cx.waker().clone()),
        }
        Poll::Pending
    }
}

/// A writer job's result: `.await` it, or [`Pending::wait`] for it on a
/// thread that may block. Dropping it does not cancel the job, which still
/// runs and commits; only its result is lost.
#[must_use = "the job runs anyway; drop the result explicitly if it is not needed"]
pub struct Pending<T, E> {
    rx: Receiver<Result<T, E>>,
    _error: PhantomData<fn() -> E>,
}

impl<T, E: From<StoreError>> Pending<T, E> {
    pub(crate) fn new(rx: Receiver<Result<T, E>>) -> Self {
        Self {
            rx,
            _error: PhantomData,
        }
    }

    /// Blocks until the job has run. Never call it on the writer thread.
    pub fn wait(self) -> Result<T, E> {
        self.rx
            .wait()
            .unwrap_or_else(|| Err(StoreError::WriterStopped.into()))
    }
}

impl<T, E: From<StoreError>> Future for Pending<T, E> {
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.rx
            .poll_value(cx)
            .map(|value| value.unwrap_or_else(|| Err(StoreError::WriterStopped.into())))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    use super::*;

    struct CountWakes(AtomicUsize);

    impl Wake for CountWakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_value_sent_from_another_thread_is_waited_for() {
        let (tx, rx) = channel::<Result<u8, StoreError>>();
        let sender = std::thread::spawn(move || tx.send(Ok(7)));
        assert_eq!(Pending::new(rx).wait().unwrap(), 7);
        sender.join().unwrap();
    }

    #[test]
    fn a_dropped_sender_means_the_writer_stopped() {
        let (tx, rx) = channel::<Result<u8, StoreError>>();
        drop(tx);
        assert!(matches!(
            Pending::new(rx).wait(),
            Err(StoreError::WriterStopped)
        ));
    }

    #[test]
    fn polling_registers_the_waker_and_send_wakes_it() {
        let (tx, rx) = channel::<Result<u8, StoreError>>();
        let mut pending = Pending::new(rx);
        let counter = Arc::new(CountWakes(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counter));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut pending).poll(&mut cx).is_pending());
        tx.send(Ok(3));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        match Pin::new(&mut pending).poll(&mut cx) {
            Poll::Ready(Ok(3)) => {}
            other => panic!("expected the value, got {other:?}"),
        }
    }
}
