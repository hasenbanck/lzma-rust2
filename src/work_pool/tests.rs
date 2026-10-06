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
        .dispatch_next_work(&mut |_| panic!("must not read input while outstanding work is full"))
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
