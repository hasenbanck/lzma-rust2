use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

use crate::recover_lock;

/// A work-stealing queue that supports multiple workers taking work from a shared queue.
///
/// Will be removed once core::sync::mpsc is stable.
pub(crate) struct WorkStealingQueue<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    state: Mutex<QueueState<T>>,
    condvar: Condvar,
}

struct QueueState<T> {
    queue: VecDeque<T>,
    closed: bool,
}

impl<T> WorkStealingQueue<T> {
    /// Creates a new work-stealing queue.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(QueueState {
                    queue: VecDeque::new(),
                    closed: false,
                }),
                condvar: Condvar::new(),
            }),
        }
    }

    /// Creates a worker handle that can steal work from this queue.
    pub(crate) fn worker(&self) -> WorkerHandle<T> {
        WorkerHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Pushes work to the queue. Returns false if the queue is closed.
    pub(crate) fn push(&self, item: T) -> bool {
        let mut state = recover_lock(self.inner.state.lock());
        if state.closed {
            return false;
        }
        state.queue.push_back(item);
        drop(state);

        // Notify one waiting worker
        self.inner.condvar.notify_one();
        true
    }

    /// Closes the queue, preventing new work from being added.
    /// Workers will continue to process remaining work until the queue is empty.
    pub(crate) fn close(&self) {
        // Serialize with the worker predicate check and Condvar::wait so that
        // closure cannot notify in the gap before a worker goes to sleep.
        recover_lock(self.inner.state.lock()).closed = true;
        // Wake up all waiting workers so they can check the closed status
        self.inner.condvar.notify_all();
    }

    /// Release queued work after shutdown, outside the queue lock.
    pub(crate) fn discard_pending(&self) {
        let pending = core::mem::take(&mut recover_lock(self.inner.state.lock()).queue);
        drop(pending);
    }

    /// Returns the current number of items in the queue.
    pub(crate) fn len(&self) -> usize {
        recover_lock(self.inner.state.lock()).queue.len()
    }

    /// Returns true if the queue is empty.
    pub(crate) fn is_empty(&self) -> bool {
        recover_lock(self.inner.state.lock()).queue.is_empty()
    }
}

impl<T> Default for WorkStealingQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A handle for workers to steal work from the queue.
pub(crate) struct WorkerHandle<T> {
    inner: Arc<Inner<T>>,
}

impl<T> WorkerHandle<T> {
    /// Attempts to steal work from the queue. Blocks until work is available or the queue is closed.
    /// Returns `None` if the queue is closed and empty.
    pub(crate) fn steal(&self) -> Option<T> {
        let mut state = recover_lock(self.inner.state.lock());

        loop {
            if let Some(item) = state.queue.pop_front() {
                return Some(item);
            }

            if state.closed {
                return None;
            }

            state = recover_lock(self.inner.condvar.wait(state));
        }
    }

    /// Attempts to steal work without blocking.
    /// Returns `None` if no work is currently available.
    pub(crate) fn try_steal(&self) -> Option<T> {
        recover_lock(self.inner.state.lock()).queue.pop_front()
    }

    /// Returns `true` if the queue is closed and empty (no more work will ever be available).
    pub(crate) fn is_closed_and_empty(&self) -> bool {
        let state = recover_lock(self.inner.state.lock());
        state.closed && state.queue.is_empty()
    }
}

impl<T> Clone for WorkerHandle<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{
            atomic::{AtomicBool, AtomicU32},
            mpsc::{self, SyncSender},
        },
        thread,
        time::Duration,
    };

    use super::*;
    use crate::work_pool::{WorkPool, WorkPoolConfig};

    #[test]
    fn poisoned_queue_preserves_work_and_can_close() {
        let queue = WorkStealingQueue::new();
        assert!(queue.push(1));
        let worker = queue.worker();
        assert!(
            thread::spawn(move || {
                let _state = worker.inner.state.lock().unwrap();
                panic!("poison queue lock");
            })
            .join()
            .is_err()
        );

        assert!(queue.inner.state.is_poisoned());
        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());
        assert!(queue.push(2));
        let worker = queue.worker();
        assert_eq!(worker.try_steal(), Some(1));
        assert_eq!(worker.steal(), Some(2));
        assert!(queue.push(3));
        queue.discard_pending();
        assert!(queue.is_empty());
        queue.close();
        assert!(!queue.push(4));
        assert_eq!(worker.steal(), None);
        assert!(worker.is_closed_and_empty());
    }

    #[test]
    fn pool_drop_recovers_poisoned_queue_during_unwinding() {
        fn worker(
            queue: WorkerHandle<(u64, mpsc::Sender<()>)>,
            _results: SyncSender<(u64, ())>,
            _shutdown: Arc<AtomicBool>,
            _errors: Arc<Mutex<Option<io::Error>>>,
            _active: Arc<AtomicU32>,
        ) {
            let (_, poisoned) = queue.steal().unwrap();
            assert!(
                catch_unwind(AssertUnwindSafe(|| {
                    let _state = queue.inner.state.lock().unwrap();
                    panic!("poison queue lock");
                }))
                .is_err()
            );
            poisoned.send(()).unwrap();
        }

        let mut pool = WorkPool::new(WorkPoolConfig::new(1, 1), worker);
        let (poisoned_tx, poisoned_rx) = mpsc::channel();
        let mut poisoned_tx = Some(poisoned_tx);
        pool.dispatch_next_work(&mut |_| Ok(poisoned_tx.take().unwrap()))
            .unwrap();
        poisoned_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let panic = catch_unwind(AssertUnwindSafe(move || {
            let _pool = pool;
            panic!("caller panic");
        }))
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"caller panic"));
    }

    #[test]
    fn closing_waits_for_worker_predicate_lock() {
        let queue = WorkStealingQueue::<u8>::new();
        let worker = queue.worker();
        // Hold the lock at the point where a worker has checked the predicate
        // but has not yet entered Condvar::wait. Close must not notify here.
        let guard = queue.inner.state.lock().unwrap();
        let closing_queue = WorkStealingQueue {
            inner: Arc::clone(&queue.inner),
        };
        let (started_tx, started_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let closer = thread::spawn(move || {
            started_tx.send(()).unwrap();
            closing_queue.close();
            closed_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let closed_before_unlock = closed_rx.recv_timeout(Duration::from_millis(100));
        drop(guard);
        if matches!(closed_before_unlock, Err(mpsc::RecvTimeoutError::Timeout)) {
            closed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        closer.join().unwrap();
        assert!(
            matches!(closed_before_unlock, Err(mpsc::RecvTimeoutError::Timeout)),
            "close notified before the worker could atomically enter its wait"
        );
        assert_eq!(worker.steal(), None);
    }

    #[test]
    fn closing_preserves_queued_work_and_rejects_new_work() {
        let queue = WorkStealingQueue::new();
        let worker = queue.worker();
        assert!(queue.push(1));
        assert!(queue.push(2));
        queue.close();
        assert!(!queue.push(3));
        assert_eq!(worker.steal(), Some(1));
        assert_eq!(worker.steal(), Some(2));
        assert_eq!(worker.steal(), None);
        assert!(worker.is_closed_and_empty());
    }

    #[test]
    fn closing_releases_all_workers() {
        let queue = WorkStealingQueue::<u8>::new();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let worker = queue.worker();
                let ready = ready_tx.clone();
                let finished = finished_tx.clone();
                thread::spawn(move || {
                    ready.send(()).unwrap();
                    assert_eq!(worker.steal(), None);
                    finished.send(()).unwrap();
                })
            })
            .collect();
        drop(ready_tx);
        drop(finished_tx);
        for _ in &workers {
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        queue.close();
        for _ in &workers {
            finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn test_basic_functionality() {
        let queue = WorkStealingQueue::new();
        let worker = queue.worker();

        assert!(queue.push(1));
        assert!(queue.push(2));
        assert!(queue.push(3));

        assert_eq!(worker.steal(), Some(1));
        assert_eq!(worker.steal(), Some(2));

        assert_eq!(worker.try_steal(), Some(3));
        assert_eq!(worker.try_steal(), None);

        queue.close();
        assert!(!queue.push(4));
        assert!(worker.is_closed_and_empty());
    }

    #[test]
    fn test_multiple_workers() {
        let queue = WorkStealingQueue::new();
        let worker1 = queue.worker();
        let worker2 = queue.worker();

        for i in 0..10 {
            queue.push(i);
        }

        let mut results = Vec::new();
        while let Some(item) = worker1.try_steal() {
            results.push(item);
        }
        while let Some(item) = worker2.try_steal() {
            results.push(item);
        }

        results.sort();
        assert_eq!(results, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn test_blocking_behavior() {
        let queue = WorkStealingQueue::new();
        let worker = queue.worker();

        let queue_clone = WorkStealingQueue {
            inner: Arc::clone(&queue.inner),
        };

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            queue_clone.push(42);
            queue_clone.close();
        });

        assert_eq!(worker.steal(), Some(42));
        assert_eq!(worker.steal(), None);
    }
}
