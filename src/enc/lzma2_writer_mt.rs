use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
};

use super::LZMA2Writer;
use crate::{
    error_invalid_input, set_error,
    work_queue::{WorkStealingQueue, WorkerHandle},
    ByteWriter, LZMA2Options,
};

/// A work unit for a worker thread.
/// Contains the sequence number and the raw uncompressed data.
type WorkUnit = (u64, Vec<u8>);

/// A result unit from a worker thread.
/// Contains the sequence number and the compressed data.
type ResultUnit = (u64, Vec<u8>);

enum State {
    /// Actively accepting input data and dispatching work to threads.
    Writing,
    /// No more input data will come. We are now waiting for the remaining
    /// work to be completed by the worker threads.
    Finishing,
    /// All data has been compressed and written. The stream is finished.
    Finished,
    /// A fatal error occurred in either the writer or a worker thread.
    Error,
}

/// A multi-threaded LZMA2 compressor.
pub struct LZMA2WriterMT<W: Write> {
    inner: Option<W>,
    options: LZMA2Options,
    stream_size: usize,
    result_rx: Receiver<ResultUnit>,
    result_tx: Sender<ResultUnit>,
    current_work_unit: Vec<u8>,
    next_sequence_to_dispatch: u64,
    next_sequence_to_write: u64,
    last_sequence_id: Option<u64>,
    out_of_order_chunks: BTreeMap<u64, Vec<u8>>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    state: State,
    work_queue: WorkStealingQueue<WorkUnit>,
    active_workers: Arc<AtomicU32>,
    max_workers: u32,
    worker_handles: Vec<thread::JoinHandle<()>>,
}

impl<W: Write> LZMA2WriterMT<W> {
    /// Creates a new multi-threaded LZMA2 writer.
    ///
    /// - `inner`: The writer to write compressed data to.
    /// - `options`: The LZMA2 options used for compressing. Stream size must be set when using the
    ///   multi-threaded encoder. If you need just one stream, then use the single-threaded encoder.
    /// - `num_workers`: The maximum number of worker threads for compression.
    ///   Currently capped at 256 Threads.
    pub fn new(inner: W, options: LZMA2Options, num_workers: u32) -> crate::Result<Self> {
        let max_workers = num_workers.clamp(1, 256);

        let stream_size = match options.stream_size {
            None => return Err(error_invalid_input("stream size must be set")),
            Some(stream_size) => stream_size.get().max(options.lzma_options.dict_size as u64),
        };

        let stream_size = usize::try_from(stream_size)
            .map_err(|_| error_invalid_input("stream size bigger than usize"))?;

        let work_queue = WorkStealingQueue::new();
        let (result_tx, result_rx) = mpsc::channel::<ResultUnit>();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let error_store = Arc::new(Mutex::new(None));
        let active_workers = Arc::new(AtomicU32::new(0));

        let mut writer = Self {
            inner: Some(inner),
            options,
            stream_size,
            result_rx,
            result_tx,
            current_work_unit: Vec::with_capacity(stream_size),
            next_sequence_to_dispatch: 0,
            next_sequence_to_write: 0,
            last_sequence_id: None,
            out_of_order_chunks: BTreeMap::new(),
            shutdown_flag,
            error_store,
            state: State::Writing,
            work_queue,
            active_workers,
            max_workers,
            worker_handles: Vec::new(),
        };

        writer.spawn_worker_thread();

        Ok(writer)
    }

    fn spawn_worker_thread(&mut self) {
        let worker_handle = self.work_queue.worker();
        let result_tx = self.result_tx.clone();
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let error_store = Arc::clone(&self.error_store);
        let active_workers = Arc::clone(&self.active_workers);
        let mut options = self.options.clone();

        options.lzma_options.preset_dict = None;

        let handle = thread::spawn(move || {
            worker_thread_logic(
                worker_handle,
                result_tx,
                options,
                shutdown_flag,
                error_store,
                active_workers,
            );
        });

        self.worker_handles.push(handle);
    }

    /// Sends the current work unit to the workers, blocking if the queue is full.
    fn send_work_unit(&mut self) -> io::Result<()> {
        if self.current_work_unit.is_empty() {
            return Ok(());
        }

        while self.work_queue.len() >= 4 {
            if let Some(chunk) = self.get_next_compressed_chunk(true)? {
                self.inner
                    .as_mut()
                    .expect("inner is empty")
                    .write_all(&chunk)?;
            } else {
                // If we get None, the stream is finished or errored. We can't send more work.
                if !matches!(self.state, State::Writing) {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Stream has been closed or is in an error state.",
                    ));
                }
            }
        }

        let work_unit = core::mem::take(&mut self.current_work_unit);

        if !self
            .work_queue
            .push((self.next_sequence_to_dispatch, work_unit))
        {
            // Queue is closed, this indicates shutdown.
            self.state = State::Error;
            let err = io::Error::new(io::ErrorKind::BrokenPipe, "Worker threads have shut down");
            set_error(err, &self.error_store, &self.shutdown_flag);

            return Err(self
                .error_store
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| io::Error::other("Failed to push to work queue")));
        }

        // We spawn a new thread if we have work queued, no available workers, and haven't reached
        // the maximal allowed parallelism yet.
        let spawned_workers = self.worker_handles.len() as u32;
        let active_workers = self.active_workers.load(Ordering::Acquire);
        let queue_len = self.work_queue.len();

        if queue_len > 0 && active_workers == spawned_workers && spawned_workers < self.max_workers
        {
            self.spawn_worker_thread();
        }

        self.next_sequence_to_dispatch += 1;
        Ok(())
    }

    /// Pulls the next available compressed data chunk, managing state transitions.
    ///
    /// The `blocking` parameter controls whether to wait for a result or return immediately.
    fn get_next_compressed_chunk(&mut self, blocking: bool) -> io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(result) = self
                .out_of_order_chunks
                .remove(&self.next_sequence_to_write)
            {
                self.next_sequence_to_write += 1;
                return Ok(Some(result));
            }

            if let Some(err) = self.error_store.lock().unwrap().take() {
                self.state = State::Error;
                return Err(err);
            }

            match self.state {
                State::Writing => {
                    let recv_result = if blocking {
                        self.result_rx
                            .recv()
                            .map_err(|_| mpsc::TryRecvError::Disconnected)
                    } else {
                        self.result_rx.try_recv()
                    };

                    match recv_result {
                        Ok((seq, result)) => {
                            if seq == self.next_sequence_to_write {
                                self.next_sequence_to_write += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(seq, result);
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            // This only happens when blocking is false.
                            return Ok(None);
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            // All workers are gone. Transition to Finishing to handle
                            // any remaining out-of-order chunks.
                            self.state = State::Finishing;
                        }
                    }
                }
                State::Finishing => {
                    if let Some(last_seq) = self.last_sequence_id {
                        if self.next_sequence_to_write > last_seq
                            && self.out_of_order_chunks.is_empty()
                        {
                            self.state = State::Finished;
                            continue;
                        }
                    }

                    match self.result_rx.recv() {
                        Ok((seq, result)) => {
                            if seq == self.next_sequence_to_write {
                                self.next_sequence_to_write += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(seq, result);
                            }
                        }
                        Err(_) => {
                            // If we get here, it means no more results will ever arrive.
                            // Let's check if the chunks we have are sufficient.
                            if let Some(last_seq) = self.last_sequence_id {
                                if self.next_sequence_to_write <= last_seq
                                    && self.out_of_order_chunks.is_empty()
                                {
                                    // We expected more chunks, but the workers are gone and the
                                    // out-of-order buffer is empty. This is a real error.
                                    self.state = State::Error;
                                    let err = io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        format!("A compressed chunk was lost. Expected up to seq {}, but only got up to {}", last_seq, self.next_sequence_to_write.saturating_sub(1)),
                                    );
                                    set_error(err, &self.error_store, &self.shutdown_flag);
                                }
                            }
                            // Otherwise, allow the loop to continue to drain the map.
                        }
                    }
                }
                State::Finished => {
                    return Ok(None);
                }
                State::Error => {
                    return Err(self.error_store.lock().unwrap().take().unwrap_or_else(|| {
                        io::Error::other("Compression failed with an unknown error")
                    }));
                }
            }
        }
    }

    /// Returns a mutable reference to the underlying writer.
    pub fn inner(&mut self) -> &mut W {
        self.inner.as_mut().expect("inner is empty")
    }

    /// Finishes the compression and returns the underlying writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.send_work_unit()?;

        // No data was provided to compress.
        if self.next_sequence_to_dispatch == 0 {
            let mut inner = self.inner.take().expect("inner is empty");
            inner.write_u8(0x00)?;
            inner.flush()?;

            self.shutdown_flag.store(true, Ordering::Release);
            self.work_queue.close();

            return Ok(inner);
        }

        self.last_sequence_id = Some(self.next_sequence_to_dispatch.saturating_sub(1));
        self.state = State::Finishing;

        while let Some(chunk) = self.get_next_compressed_chunk(true)? {
            self.inner
                .as_mut()
                .expect("inner is empty")
                .write_all(&chunk)?;
        }

        let mut inner = self.inner.take().expect("inner is empty");

        inner.write_u8(0x00)?;
        inner.flush()?;

        self.shutdown_flag.store(true, Ordering::Release);
        self.work_queue.close();

        Ok(inner)
    }
}

/// The logic for a single worker thread.
fn worker_thread_logic(
    worker_handle: WorkerHandle<WorkUnit>,
    result_tx: Sender<ResultUnit>,
    options: LZMA2Options,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    active_workers: Arc<AtomicU32>,
) {
    while !shutdown_flag.load(Ordering::Acquire) {
        let (seq, work_unit_data) = match worker_handle.steal() {
            Some(work) => {
                active_workers.fetch_add(1, Ordering::Release);
                work
            }
            None => {
                // No more work available and queue is closed
                break;
            }
        };

        let mut compressed_buffer = Vec::new();

        let mut writer = LZMA2Writer::new(&mut compressed_buffer, &options);

        let result = match writer.write_all(&work_unit_data) {
            Ok(_) => match writer.flush() {
                Ok(_) => compressed_buffer,
                Err(error) => {
                    active_workers.fetch_sub(1, Ordering::Release);
                    set_error(error, &error_store, &shutdown_flag);
                    return;
                }
            },
            Err(error) => {
                active_workers.fetch_sub(1, Ordering::Release);
                set_error(error, &error_store, &shutdown_flag);
                return;
            }
        };

        if result_tx.send((seq, result)).is_err() {
            active_workers.fetch_sub(1, Ordering::Release);
            return;
        }

        active_workers.fetch_sub(1, Ordering::Release);
    }
}

impl<W: Write> Write for LZMA2WriterMT<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        if !matches!(self.state, State::Writing) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot write after finishing",
            ));
        }

        let mut total_written = 0;
        let mut remaining_buf = buf;

        while !remaining_buf.is_empty() {
            let stream_remaining = self
                .stream_size
                .saturating_sub(self.current_work_unit.len());
            let to_write = remaining_buf.len().min(stream_remaining);

            if to_write > 0 {
                self.current_work_unit
                    .extend_from_slice(&remaining_buf[..to_write]);
                total_written += to_write;
                remaining_buf = &remaining_buf[to_write..];
            }

            if self.current_work_unit.len() >= self.stream_size {
                self.send_work_unit()?;
            }

            while let Some(chunk) = self.get_next_compressed_chunk(false)? {
                self.inner
                    .as_mut()
                    .expect("inner is empty")
                    .write_all(&chunk)?;
            }
        }

        Ok(total_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.current_work_unit.is_empty() {
            self.send_work_unit()?;
        }

        let sequence_to_wait = self.next_sequence_to_dispatch;

        while self.next_sequence_to_write < sequence_to_wait {
            match self.get_next_compressed_chunk(true)? {
                Some(chunk) => {
                    self.inner
                        .as_mut()
                        .expect("inner is empty")
                        .write_all(&chunk)?;
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Compression stream ended unexpectedly during flush",
                    ));
                }
            }
        }

        self.inner.as_mut().expect("inner is empty").flush()
    }
}

impl<W: Write> Drop for LZMA2WriterMT<W> {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.work_queue.close();
        // Worker threads will exit when the work queue is closed.
        // JoinHandles will be dropped, which is fine since we set the shutdown flag.
    }
}
