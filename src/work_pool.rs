use std::{
    collections::BTreeMap,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread,
    time::Duration,
};

/// Interval for checking worker errors while waiting for results.
const ERROR_CHECK_INTERVAL: Duration = Duration::from_millis(100);

use crate::{
    recover_lock, set_error,
    work_queue::{WorkStealingQueue, WorkerHandle},
};

/// Cooperative encoder cancellation, carried through the writer's I/O result.
#[derive(Debug)]
pub struct EncoderCancelled;

impl std::fmt::Display for EncoderCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("encoder cancelled")
    }
}

impl std::error::Error for EncoderCancelled {}

/// Configuration for a work pool.
#[derive(Debug, Clone)]
pub(crate) struct WorkPoolConfig {
    pub(crate) num_workers: u32,
    pub(crate) num_work: u64,
}

impl WorkPoolConfig {
    pub(crate) fn new(num_workers: u32, num_work: u64) -> Self {
        Self {
            num_workers,
            num_work,
        }
    }
}

/// States for the work pool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WorkPoolState {
    /// Actively accepting work and dispatching to threads.
    Dispatching,
    /// No more work will be submitted, draining existing work.
    Draining,
    /// All work completed.
    Finished,
    /// An error occurred.
    Error,
}

pub(crate) type WorkerFunction<W, R> = fn(
    WorkerHandle<(u64, W)>,
    SyncSender<(u64, R)>,
    Arc<AtomicBool>,
    Arc<Mutex<Option<io::Error>>>,
    Arc<AtomicU32>,
);

/// A generic work pool for the multi threading reader and writer.
pub(crate) struct WorkPool<W, R> {
    work_queue: WorkStealingQueue<(u64, W)>,
    result_rx: Option<Receiver<(u64, R)>>,
    result_tx: SyncSender<(u64, R)>,
    next_index_to_dispatch: u64,
    next_index_to_return: u64,
    out_of_order_results: BTreeMap<u64, R>,
    shutdown_flag: Arc<AtomicBool>,
    cancellation: Option<Arc<AtomicBool>>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    state: WorkPoolState,
    active_workers: Arc<AtomicU32>,
    num_workers: u32,
    num_work: u64,
    worker_handles: Vec<thread::JoinHandle<()>>,
    worker_fn: WorkerFunction<W, R>,
}

impl<W, R> WorkPool<W, R>
where
    W: Send + 'static,
    R: Send + 'static,
{
    /// Create a new work pool that spawns workers using the provided worker function.
    pub(crate) fn new(config: WorkPoolConfig, worker_fn: WorkerFunction<W, R>) -> Self {
        let (result_tx, result_rx) = mpsc::sync_channel::<(u64, R)>(1);

        let mut pool = Self {
            work_queue: WorkStealingQueue::new(),
            result_rx: Some(result_rx),
            result_tx,
            next_index_to_dispatch: 0,
            next_index_to_return: 0,
            out_of_order_results: BTreeMap::new(),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            cancellation: None,
            error_store: Arc::new(Mutex::new(None)),
            state: WorkPoolState::Dispatching,
            active_workers: Arc::new(AtomicU32::new(0)),
            num_workers: config.num_workers.clamp(1, 256),
            num_work: config.num_work,
            worker_handles: Vec::new(),
            worker_fn,
        };

        pool.spawn_worker_thread();

        pool
    }

    pub(crate) fn next_index_to_dispatch(&self) -> u64 {
        self.next_index_to_dispatch
    }

    /// Includes queued jobs, active workers and results waiting for their turn.
    pub(crate) fn is_full(&self) -> bool {
        self.next_index_to_dispatch - self.next_index_to_return > u64::from(self.num_workers)
    }

    /// Report worker errors and shut down the pool, joining all workers on failure.
    pub(crate) fn check_error_and_abort(&mut self) -> io::Result<()> {
        let error = recover_lock(self.error_store.lock()).take();
        if let Some(error) = error {
            self.abort();
            return Err(error);
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            self.abort();
            return Err(io::Error::other(EncoderCancelled));
        }
        if self.state == WorkPoolState::Error {
            return Err(io::Error::other("work pool has failed"));
        }
        Ok(())
    }

    pub(crate) fn set_cancellation(&mut self, flag: Arc<AtomicBool>) {
        self.cancellation = Some(flag);
    }

    /// Submit work to the pool. Returns `false` if there is no more work to work on.
    pub(crate) fn dispatch_next_work<F>(&mut self, next_work_function: &mut F) -> io::Result<bool>
    where
        F: FnMut(u64) -> io::Result<W>,
    {
        self.check_error_and_abort()?;
        if self.state != WorkPoolState::Dispatching {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "work pool is closed",
            ));
        }
        let next_index = self.next_index_to_dispatch;

        if next_index >= self.num_work {
            // No more members to dispatch.
            return Ok(false);
        }

        if self.is_full() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "work pool is full",
            ));
        }

        let work = next_work_function(next_index)?;

        if !self.work_queue.push((next_index, work)) {
            // Queue is closed, this indicates shutdown.
            self.state = WorkPoolState::Error;
            set_error(
                io::Error::new(io::ErrorKind::BrokenPipe, "worker threads have shut down"),
                &self.error_store,
                &self.shutdown_flag,
            );
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "worker threads have shut down",
            ));
        }

        self.maybe_spawn_worker();

        self.next_index_to_dispatch += 1;

        Ok(true)
    }

    /// Try to get the next result in sequence order. Returns None if no result is ready.
    pub(crate) fn try_get_result(&mut self) -> io::Result<Option<R>> {
        self.check_error_and_abort()?;
        // Check if we have the next result in sequence.
        if let Some(result) = self.out_of_order_results.remove(&self.next_index_to_return) {
            self.next_index_to_return += 1;
            return Ok(Some(result));
        }

        // Try to receive a result without blocking.
        let Some(result_rx) = &self.result_rx else {
            return Ok(None);
        };
        match result_rx.try_recv() {
            Ok((seq, result)) => {
                if seq == self.next_index_to_return {
                    self.next_index_to_return += 1;
                    Ok(Some(result))
                } else {
                    self.out_of_order_results.insert(seq, result);
                    Ok(None)
                }
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                if matches!(self.state, WorkPoolState::Dispatching) {
                    self.state = WorkPoolState::Draining;
                }
                Ok(None)
            }
        }
    }

    /// Get the next result of the already dispatched work, blocking until available.
    ///
    /// Returns `None` once everything that was dispatched has been returned. This
    /// never asks for new work, so more work can be dispatched afterwards.
    pub(crate) fn get_dispatched_result(&mut self) -> io::Result<Option<R>> {
        loop {
            self.check_error_and_abort()?;

            if let Some(result) = self.out_of_order_results.remove(&self.next_index_to_return) {
                self.next_index_to_return += 1;
                return Ok(Some(result));
            }

            if self.next_index_to_return == self.next_index_to_dispatch {
                if self.state == WorkPoolState::Draining {
                    self.state = WorkPoolState::Finished;
                    self.shutdown();
                }
                return Ok(None);
            }

            let Some(result_rx) = &self.result_rx else {
                return Ok(None);
            };
            match result_rx.recv_timeout(ERROR_CHECK_INTERVAL) {
                Ok((seq, result)) => {
                    if seq == self.next_index_to_return {
                        self.next_index_to_return += 1;
                        return Ok(Some(result));
                    } else {
                        self.out_of_order_results.insert(seq, result);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.check_error_and_abort()?;
                    self.abort();
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "worker threads have shut down with work outstanding",
                    ));
                }
            }
        }
    }

    /// Get the next result in sequence order, blocking until available.
    pub(crate) fn get_result<F>(&mut self, mut next_work_function: F) -> io::Result<Option<R>>
    where
        F: FnMut(u64) -> io::Result<W>,
    {
        loop {
            self.check_error_and_abort()?;
            // Always check for already-received results first.
            if let Some(result) = self.out_of_order_results.remove(&self.next_index_to_return) {
                self.next_index_to_return += 1;
                return Ok(Some(result));
            }

            match self.state {
                WorkPoolState::Dispatching => {
                    // First, always try to receive a result without blocking.
                    // This keeps the pipeline moving and avoids unnecessary blocking.
                    let Some(result_rx) = &self.result_rx else {
                        return Ok(None);
                    };
                    match result_rx.try_recv() {
                        Ok((seq, result)) => {
                            if seq == self.next_index_to_return {
                                self.next_index_to_return += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_results.insert(seq, result);
                                continue; // Loop again to check the out_of_order_results.
                            }
                        }
                        Err(TryRecvError::Disconnected) => {
                            // All workers are done.
                            self.state = WorkPoolState::Draining;
                            continue;
                        }
                        Err(TryRecvError::Empty) => {
                            // No results are ready. Now, we can consider dispatching more work.
                        }
                    }

                    // If the work queue has capacity, try to read more from the source.
                    if !self.is_full() && self.work_queue.len() < self.num_workers as usize {
                        match self.dispatch_next_work(&mut next_work_function) {
                            Ok(true) => {
                                // Successfully read and dispatched a chunk, loop to continue.
                                continue;
                            }
                            Ok(false) => {
                                // No more work to dispatch.
                                self.finish();
                                continue;
                            }
                            Err(error) => {
                                set_error(error, &self.error_store, &self.shutdown_flag);
                                self.state = WorkPoolState::Error;
                                continue;
                            }
                        }
                    }

                    // Now we MUST wait for a result to make progress.
                    loop {
                        let Some(result_rx) = &self.result_rx else {
                            return Ok(None);
                        };
                        match result_rx.recv_timeout(ERROR_CHECK_INTERVAL) {
                            Ok((seq, result)) => {
                                if seq == self.next_index_to_return {
                                    self.next_index_to_return += 1;
                                    return Ok(Some(result));
                                } else {
                                    self.out_of_order_results.insert(seq, result);
                                    // We've made progress, loop to check the out_of_order_results.
                                    break;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                self.check_error_and_abort()?;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                // All workers are done.
                                self.state = WorkPoolState::Draining;
                                break;
                            }
                        }
                    }
                }
                WorkPoolState::Draining => {
                    if self.next_index_to_return == self.next_index_to_dispatch {
                        self.state = WorkPoolState::Finished;
                        self.shutdown();
                        continue;
                    }

                    // In Draining state, we only wait for results.
                    loop {
                        let Some(result_rx) = &self.result_rx else {
                            return Ok(None);
                        };
                        match result_rx.recv_timeout(ERROR_CHECK_INTERVAL) {
                            Ok((seq, result)) => {
                                if seq == self.next_index_to_return {
                                    self.next_index_to_return += 1;
                                    return Ok(Some(result));
                                } else {
                                    self.out_of_order_results.insert(seq, result);
                                    break;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                self.check_error_and_abort()?;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                // All workers finished, and channel is empty. We are done.
                                self.state = WorkPoolState::Finished;
                                break;
                            }
                        }
                    }
                }
                WorkPoolState::Finished => {
                    return Ok(None);
                }
                WorkPoolState::Error => {
                    return Err(io::Error::other("work pool has failed"));
                }
            }
        }
    }

    /// Mark that no more work will be submitted and begin draining.
    pub(crate) fn finish(&mut self) {
        if matches!(self.state, WorkPoolState::Dispatching) {
            self.state = WorkPoolState::Draining;
            self.work_queue.close();
        }
    }

    /// Check if the work queue is empty.
    pub(crate) fn is_work_queue_empty(&self) -> bool {
        self.work_queue.is_empty()
    }

    /// Get the current state.
    pub(crate) fn state(&self) -> WorkPoolState {
        self.state
    }

    fn spawn_worker_thread(&mut self) {
        let worker_handle = self.work_queue.worker();
        let result_tx = self.result_tx.clone();
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let error_store = Arc::clone(&self.error_store);
        let active_workers = Arc::clone(&self.active_workers);
        let worker_fn = self.worker_fn;

        let handle = thread::Builder::new().spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                worker_fn(
                    worker_handle,
                    result_tx,
                    Arc::clone(&shutdown_flag),
                    Arc::clone(&error_store),
                    active_workers,
                )
            }));
            if result.is_err() {
                set_error(
                    io::Error::other("worker thread panicked"),
                    &error_store,
                    &shutdown_flag,
                );
            }
        });

        match handle {
            Ok(handle) => self.worker_handles.push(handle),
            Err(error) => set_error(error, &self.error_store, &self.shutdown_flag),
        }
    }

    fn maybe_spawn_worker(&mut self) {
        let spawned_workers = self.worker_handles.len() as u32;
        let active_workers = self.active_workers.load(Ordering::Acquire);
        let queue_len = self.work_queue.len();

        // Spawn another worker when more items are queued than there are idle ones. A parked
        // worker that has not stolen its item yet still counts as idle.
        let idle_workers = spawned_workers.saturating_sub(active_workers) as usize;
        if queue_len > idle_workers && spawned_workers < self.num_workers {
            self.spawn_worker_thread();
        }
    }
}

impl<W, R> WorkPool<W, R> {
    pub(crate) fn abort(&mut self) {
        self.state = WorkPoolState::Error;
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(result_rx) = self.result_rx.take() else {
            return;
        };

        self.shutdown_flag.store(true, Ordering::Release);
        self.work_queue.discard_pending();
        self.work_queue.close();

        // Disconnect before joining: workers may be blocked sending a result.
        drop(result_rx);
        for handle in self.worker_handles.drain(..) {
            let _ = handle.join();
        }
        self.out_of_order_results.clear();
    }
}

impl<W, R> Drop for WorkPool<W, R> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEADLINE: Duration = Duration::from_secs(5);

    #[test]
    fn stored_worker_error_survives_poisoned_error_lock() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(1, 0), worker);
        let errors = Arc::clone(&pool.error_store);
        assert!(
            thread::spawn(move || {
                let mut guard = errors.lock().unwrap();
                *guard = Some(io::Error::new(io::ErrorKind::InvalidData, "bad block"));
                panic!("poison error lock");
            })
            .join()
            .is_err()
        );

        let error = pool.check_error_and_abort().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "bad block");
        assert!(pool.worker_handles.is_empty());
    }

    #[test]
    fn worker_panic_is_reported_after_poisoning_error_lock() {
        fn worker(
            _queue: WorkerHandle<(u64, ())>,
            _results: SyncSender<(u64, ())>,
            _shutdown: Arc<AtomicBool>,
            errors: Arc<Mutex<Option<io::Error>>>,
            _active: Arc<AtomicU32>,
        ) {
            let _guard = errors.lock().unwrap();
            panic!("poison error lock");
        }

        let mut pool = WorkPool::new(WorkPoolConfig::new(1, 0), worker);
        assert!(pool.worker_handles.pop().unwrap().join().is_ok());
        assert_eq!(
            pool.check_error_and_abort().unwrap_err().to_string(),
            "worker thread panicked"
        );
        assert_eq!(pool.state(), WorkPoolState::Error);
    }

    struct Job {
        run: Box<dyn FnOnce() -> io::Result<u64> + Send>,
        sent: Option<mpsc::Sender<()>>,
    }

    fn worker(
        queue: WorkerHandle<(u64, Job)>,
        results: SyncSender<(u64, u64)>,
        shutdown: Arc<AtomicBool>,
        errors: Arc<Mutex<Option<io::Error>>>,
        active: Arc<AtomicU32>,
    ) {
        while !shutdown.load(Ordering::Acquire) {
            let Some((index, job)) = queue.steal() else {
                break;
            };
            active.fetch_add(1, Ordering::Release);
            let value = match (job.run)() {
                Ok(value) => value,
                Err(error) => {
                    active.fetch_sub(1, Ordering::Release);
                    set_error(error, &errors, &shutdown);
                    return;
                }
            };
            if results.send((index, value)).is_err() {
                active.fetch_sub(1, Ordering::Release);
                return;
            }
            if let Some(sent) = job.sent {
                sent.send(()).unwrap();
            }
            active.fetch_sub(1, Ordering::Release);
        }
    }

    fn submit(pool: &mut WorkPool<Job, u64>, job: Job) {
        let mut job = Some(job);
        assert!(
            pool.dispatch_next_work(&mut |_| Ok(job.take().unwrap()))
                .unwrap()
        );
    }

    #[test]
    fn stalled_first_job_bounds_input_and_reordered_results() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(2, u64::MAX), worker);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        submit(
            &mut pool,
            Job {
                run: Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(DEADLINE).unwrap();
                    Ok(0)
                }),
                sent: None,
            },
        );
        started_rx.recv_timeout(DEADLINE).unwrap();

        for value in 1..=2 {
            let (sent_tx, sent_rx) = mpsc::channel();
            submit(
                &mut pool,
                Job {
                    run: Box::new(move || Ok(value)),
                    sent: Some(sent_tx),
                },
            );
            sent_rx.recv_timeout(DEADLINE).unwrap();
            assert_eq!(pool.try_get_result().unwrap(), None);
        }
        assert!(pool.is_full());
        assert_eq!(pool.out_of_order_results.len(), 2);
        let error = pool
            .dispatch_next_work(&mut |_| {
                panic!("must not read input while outstanding work is full")
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);

        release_tx.send(()).unwrap();
        for value in 0..=2 {
            assert_eq!(pool.get_dispatched_result().unwrap(), Some(value));
        }
        assert!(!pool.is_full());
        assert_eq!(pool.get_dispatched_result().unwrap(), None);
        pool.finish();
        assert_eq!(pool.get_dispatched_result().unwrap(), None);
        assert!(pool.worker_handles.is_empty());
    }

    #[test]
    fn worker_error_stops_and_joins_pool() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(2, u64::MAX), worker);
        submit(
            &mut pool,
            Job {
                run: Box::new(|| Err(io::Error::new(io::ErrorKind::InvalidData, "bad block"))),
                sent: None,
            },
        );
        let error = pool.get_dispatched_result().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "bad block");
        assert!(pool.worker_handles.is_empty());
        assert!(
            pool.dispatch_next_work(&mut |_| panic!("failed pool accepted input"))
                .is_err()
        );
    }

    #[test]
    fn worker_panic_is_an_error_instead_of_a_missing_result_hang() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(1, u64::MAX), worker);
        submit(
            &mut pool,
            Job {
                run: Box::new(|| panic!("injected worker panic")),
                sent: None,
            },
        );
        assert_eq!(
            pool.get_dispatched_result().unwrap_err().to_string(),
            "worker thread panicked"
        );
        assert!(pool.worker_handles.is_empty());
        assert!(pool.try_get_result().is_err());
    }

    struct ExitSignal(mpsc::Sender<()>);

    impl Drop for ExitSignal {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    type Signals = (mpsc::Sender<()>, mpsc::Sender<()>);

    fn blocked_sender(
        queue: WorkerHandle<(u64, Signals)>,
        results: SyncSender<(u64, u64)>,
        _shutdown: Arc<AtomicBool>,
        _errors: Arc<Mutex<Option<io::Error>>>,
        _active: Arc<AtomicU32>,
    ) {
        let Some((_, (ready, exited))) = queue.steal() else {
            return;
        };
        let _exit = ExitSignal(exited);
        results.send((0, 0)).unwrap();
        ready.send(()).unwrap();
        // The capacity-one result channel is full. Shutdown must disconnect it
        // before joining, or this worker cannot return.
        let _ = results.send((1, 1));
    }

    #[test]
    fn drop_disconnects_blocked_result_sender_and_joins_it() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(1, u64::MAX), blocked_sender);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        let mut signals = Some((ready_tx, exited_tx));
        pool.dispatch_next_work(&mut |_| Ok(signals.take().unwrap()))
            .unwrap();
        ready_rx.recv_timeout(DEADLINE).unwrap();
        drop(pool);
        // No waiting: the worker must have exited before drop returned.
        exited_rx.try_recv().unwrap();
    }

    #[test]
    fn abort_discards_queued_work_before_worker_resumes() {
        fn waiting_worker(
            queue: WorkerHandle<(u64, Job)>,
            _results: SyncSender<(u64, u64)>,
            shutdown: Arc<AtomicBool>,
            _errors: Arc<Mutex<Option<io::Error>>>,
            _active: Arc<AtomicU32>,
        ) {
            let (_, control) = queue.steal().unwrap();
            // Park after checking shutdown, before stealing the next job.
            assert!(!shutdown.load(Ordering::Acquire));
            (control.run)().unwrap();
            if let Some((_, job)) = queue.steal() {
                (job.run)().unwrap();
            }
        }

        let mut pool = WorkPool::new(WorkPoolConfig::new(1, u64::MAX), waiting_worker);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (discarded_tx, discarded_rx) = mpsc::channel();
        submit(
            &mut pool,
            Job {
                run: Box::new(move || {
                    ready_tx.send(()).unwrap();
                    // Discarding the queued closure releases this worker. The timeout
                    // lets the old shutdown order finish and expose the extra job.
                    let _ = discarded_rx.recv_timeout(DEADLINE);
                    Ok(0)
                }),
                sent: None,
            },
        );
        ready_rx.recv_timeout(DEADLINE).unwrap();

        let processed = Arc::new(AtomicBool::new(false));
        let processed_job = Arc::clone(&processed);
        let discarded = ExitSignal(discarded_tx);
        submit(
            &mut pool,
            Job {
                run: Box::new(move || {
                    let _discarded = discarded;
                    processed_job.store(true, Ordering::SeqCst);
                    Ok(1)
                }),
                sent: None,
            },
        );

        pool.abort();
        assert!(
            !processed.load(Ordering::SeqCst),
            "aborted pool ran a queued job"
        );
    }

    #[test]
    fn repeated_abort_disconnects_blocked_sender_and_rejects_work() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(1, u64::MAX), blocked_sender);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        let mut signals = Some((ready_tx, exited_tx));
        pool.dispatch_next_work(&mut |_| Ok(signals.take().unwrap()))
            .unwrap();
        ready_rx.recv_timeout(DEADLINE).unwrap();

        pool.abort();
        exited_rx.try_recv().unwrap();
        pool.abort();
        pool.finish();
        assert!(pool.try_get_result().is_err());
        assert!(pool.get_dispatched_result().is_err());
        assert!(
            pool.get_result(|_| panic!("aborted pool read input"))
                .is_err()
        );
        assert!(
            pool.dispatch_next_work(&mut |_| panic!("aborted pool accepted work"))
                .is_err()
        );
    }

    #[test]
    fn input_error_aborts_reader_prefetch_and_joins_workers() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(2, 10), worker);
        let error = pool
            .get_result(|_| Err(io::Error::new(io::ErrorKind::UnexpectedEof, "input failed")))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert!(pool.worker_handles.is_empty());
        assert!(pool.work_queue.is_empty());
    }

    #[test]
    fn finishing_empty_pool_joins_idle_worker() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(2, 0), worker);
        pool.finish();
        assert_eq!(pool.get_dispatched_result().unwrap(), None);
        assert_eq!(pool.state(), WorkPoolState::Finished);
        assert!(pool.worker_handles.is_empty());
        pool.finish();
        assert_eq!(pool.try_get_result().unwrap(), None);
        assert_eq!(pool.get_dispatched_result().unwrap(), None);
        assert_eq!(
            pool.get_result(|_| panic!("finished pool read input"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn cancellation_rejects_work_before_calling_the_producer() {
        let flag = Arc::new(AtomicBool::new(true));
        let mut pool = WorkPool::new(WorkPoolConfig::new(2, 10), worker);
        pool.set_cancellation(flag);
        let error = pool
            .dispatch_next_work(&mut |_| panic!("cancelled producer called"))
            .unwrap_err();
        assert!(error.get_ref().unwrap().is::<EncoderCancelled>());
        assert!(pool.worker_handles.is_empty());
        assert!(pool.work_queue.is_empty());
    }

    #[test]
    fn reader_prefetch_returns_every_result_in_order() {
        let mut pool = WorkPool::new(WorkPoolConfig::new(2, 32), worker);
        let mut output = Vec::new();
        while let Some(value) = pool
            .get_result(|index| {
                Ok(Job {
                    run: Box::new(move || Ok(index)),
                    sent: None,
                })
            })
            .unwrap()
        {
            output.push(value);
            assert!(pool.next_index_to_dispatch - pool.next_index_to_return <= 3);
        }
        assert_eq!(output, (0..32).collect::<Vec<_>>());
        assert_eq!(pool.state(), WorkPoolState::Finished);
        assert!(pool.worker_handles.is_empty());
    }
}
