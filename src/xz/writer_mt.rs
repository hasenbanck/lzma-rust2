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

use super::{
    add_padding, write_xz_block_header, write_xz_index, write_xz_stream_footer,
    write_xz_stream_header, CheckType, ChecksumCalculator, FilterConfig, FilterType, IndexRecord,
};
use crate::{
    enc::{LZMA2Writer, LZMAOptions},
    error_invalid_input, set_error,
    work_queue::{WorkStealingQueue, WorkerHandle},
    LZMA2Options, Result, XZOptions,
};

/// A work unit for a worker thread.
#[derive(Debug)]
struct WorkUnit {
    sequence: u64,
    uncompressed_data: Vec<u8>,
}

/// A result unit from a worker thread.
#[derive(Debug)]
struct ResultUnit {
    sequence: u64,
    compressed_data: Vec<u8>,
    checksum: Vec<u8>,
    uncompressed_size: u64,
}

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

/// A multi-threaded XZ compressor.
pub struct XZWriterMT<W: Write> {
    inner: Option<W>,
    options: XZOptions,
    result_rx: Receiver<ResultUnit>,
    result_tx: Sender<ResultUnit>,
    current_work_unit: Vec<u8>,
    block_size: usize,
    next_sequence_to_dispatch: u64,
    next_sequence_to_write: u64,
    last_sequence_id: Option<u64>,
    out_of_order_chunks: BTreeMap<u64, ResultUnit>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    state: State,
    work_queue: WorkStealingQueue<WorkUnit>,
    active_workers: Arc<AtomicU32>,
    max_workers: u32,
    worker_handles: Vec<thread::JoinHandle<()>>,
    index_records: Vec<IndexRecord>,
    checksum_calculator: ChecksumCalculator,
    header_written: bool,
    total_uncompressed_pos: u64,
}

impl<W: Write> XZWriterMT<W> {
    /// Creates a new multi-threaded XZ writer.
    ///
    /// - `inner`: The writer to write compressed data to.
    /// - `options`: The XZ options used for compressing. Block size must be set when using the
    ///   multi-threaded encoder. If you need just one block, then use the single-threaded encoder.
    /// - `num_workers`: The maximum number of worker threads for compression.
    ///   Currently capped at 256 threads.
    pub fn new(inner: W, options: XZOptions, num_workers: u32) -> Result<Self> {
        let max_workers = num_workers.clamp(1, 256);

        if options.filters.len() > 3 {
            return Err(error_invalid_input(
                "XZ allows only at most 3 pre-filters plus LZMA2",
            ));
        }

        let block_size = match options.block_size {
            None => return Err(error_invalid_input("block size must be set")),
            Some(block_size) => block_size.get().max(options.lzma_options.dict_size as u64),
        };

        let block_size = usize::try_from(block_size)
            .map_err(|_| error_invalid_input("block size bigger than usize"))?;

        let work_queue = WorkStealingQueue::new();
        let (result_tx, result_rx) = mpsc::channel::<ResultUnit>();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let error_store = Arc::new(Mutex::new(None));
        let active_workers = Arc::new(AtomicU32::new(0));
        let checksum_calculator = ChecksumCalculator::new(options.check_type);

        let mut writer = Self {
            inner: Some(inner),
            options,
            result_rx,
            result_tx,
            current_work_unit: Vec::with_capacity(block_size.min(1024 * 1024)),
            block_size,
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
            index_records: Vec::new(),
            checksum_calculator,
            header_written: false,
            total_uncompressed_pos: 0,
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
        let lzma_options = self.options.lzma_options.clone();
        let check_type = self.options.check_type;

        let handle = thread::spawn(move || {
            worker_thread_logic(
                worker_handle,
                result_tx,
                lzma_options,
                check_type,
                shutdown_flag,
                error_store,
                active_workers,
            );
        });

        self.worker_handles.push(handle);
    }

    fn write_stream_header(&mut self) -> Result<()> {
        if self.header_written {
            return Ok(());
        }

        let inner = self.inner.as_mut().expect("inner is empty");
        write_xz_stream_header(inner, self.options.check_type)?;
        self.header_written = true;

        Ok(())
    }

    fn write_block_header(&mut self, _block_uncompressed_size: u64) -> Result<u64> {
        // Add LZMA2 filter to the list
        let mut filters = self.options.filters.clone();
        filters.push(FilterConfig {
            filter_type: FilterType::LZMA2,
            property: 0,
        });

        let inner = self.inner.as_mut().expect("inner is empty");
        write_xz_block_header(inner, &filters, self.options.lzma_options.dict_size)
    }

    /// Sends the current work unit to the workers, blocking if the queue is full.
    fn send_work_unit(&mut self) -> Result<()> {
        if self.current_work_unit.is_empty() {
            return Ok(());
        }

        // Ensure stream header is written before any blocks
        self.write_stream_header()?;

        while self.work_queue.len() >= 4 {
            if let Some(result) = self.get_next_compressed_chunk(true)? {
                self.write_compressed_block(
                    result.compressed_data,
                    result.checksum,
                    result.uncompressed_size,
                )?;
            } else {
                // If we get None, the stream is finished or errored. We can't send more work.
                if !matches!(self.state, State::Writing) {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "stream has been closed or is in an error state.",
                    ));
                }
            }
        }

        let work_unit = core::mem::take(&mut self.current_work_unit);

        let work_unit = WorkUnit {
            sequence: self.next_sequence_to_dispatch,
            uncompressed_data: work_unit,
        };

        if !self.work_queue.push(work_unit) {
            // Queue is closed, this indicates shutdown.
            self.state = State::Error;
            let err = io::Error::new(io::ErrorKind::BrokenPipe, "worker threads have shut down");
            set_error(err, &self.error_store, &self.shutdown_flag);

            return Err(self
                .error_store
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| io::Error::other("failed to push to work queue")));
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
    fn get_next_compressed_chunk(&mut self, blocking: bool) -> Result<Option<ResultUnit>> {
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
                        Ok(result) => {
                            if result.sequence == self.next_sequence_to_write {
                                self.next_sequence_to_write += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(result.sequence, result);
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
                        Ok(result) => {
                            if result.sequence == self.next_sequence_to_write {
                                self.next_sequence_to_write += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(result.sequence, result);
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
                        io::Error::other("compression failed with an unknown error")
                    }));
                }
            }
        }
    }

    fn write_compressed_block(
        &mut self,
        compressed_data: Vec<u8>,
        checksum: Vec<u8>,
        block_uncompressed_size: u64,
    ) -> Result<()> {
        let block_header_size = self.write_block_header(block_uncompressed_size)?;

        let data_size = compressed_data.len() as u64;
        let padding_needed = (4 - (data_size % 4)) % 4;

        let inner = self.inner.as_mut().expect("inner is empty");
        inner.write_all(&compressed_data)?;

        add_padding(inner, padding_needed as usize)?;

        inner.write_all(&checksum)?;

        let unpadded_size = block_header_size + data_size + self.options.check_type.checksum_size();
        self.index_records.push(IndexRecord {
            unpadded_size,
            uncompressed_size: block_uncompressed_size,
        });

        self.total_uncompressed_pos += block_uncompressed_size;

        Ok(())
    }

    /// Consume the XZWriterMT and return the inner writer.
    pub fn into_inner(mut self) -> W {
        self.inner.take().expect("inner is empty")
    }

    #[inline(always)]
    fn write_index(&mut self) -> Result<()> {
        let inner = self.inner.as_mut().expect("inner is empty");
        write_xz_index(inner, &self.index_records)
    }

    #[inline(always)]
    fn write_stream_footer(&mut self) -> Result<()> {
        let inner = self.inner.as_mut().expect("inner is empty");
        write_xz_stream_footer(inner, &self.index_records, self.options.check_type)
    }

    /// Finishes the compression and returns the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.write_stream_header()?;

        self.send_work_unit()?;

        // If no data was provided to compress, write an empty XZ file.
        if self.next_sequence_to_dispatch == 0 {
            // Write empty index and footer
            self.write_index()?;
            self.write_stream_footer()?;

            let mut inner = self.inner.take().expect("inner is empty");
            inner.flush()?;

            self.shutdown_flag.store(true, Ordering::Release);
            self.work_queue.close();

            return Ok(inner);
        }

        self.last_sequence_id = Some(self.next_sequence_to_dispatch.saturating_sub(1));
        self.state = State::Finishing;

        while let Some(result) = self.get_next_compressed_chunk(true)? {
            self.write_compressed_block(
                result.compressed_data,
                result.checksum,
                result.uncompressed_size,
            )?;
        }

        self.write_index()?;
        self.write_stream_footer()?;

        let mut inner = self.inner.take().expect("inner is empty");
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
    lzma_options: LZMAOptions,
    check_type: CheckType,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    active_workers: Arc<AtomicU32>,
) {
    while !shutdown_flag.load(Ordering::Acquire) {
        let work_unit = match worker_handle.steal() {
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
        let uncompressed_size = work_unit.uncompressed_data.len() as u64;

        let mut checksum_calculator = ChecksumCalculator::new(check_type);
        checksum_calculator.update(&work_unit.uncompressed_data);
        let checksum = checksum_calculator.finalize_to_bytes();

        let options = LZMA2Options {
            lzma_options: lzma_options.clone(),
            ..Default::default()
        };

        let mut writer = LZMA2Writer::new(&mut compressed_buffer, options);
        let compressed_data = match writer.write_all(&work_unit.uncompressed_data) {
            Ok(_) => match writer.finish() {
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

        let result = ResultUnit {
            sequence: work_unit.sequence,
            compressed_data,
            checksum,
            uncompressed_size,
        };

        if result_tx.send(result).is_err() {
            active_workers.fetch_sub(1, Ordering::Release);
            return;
        }

        active_workers.fetch_sub(1, Ordering::Release);
    }
}

impl<W: Write> Write for XZWriterMT<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        if !matches!(self.state, State::Writing) {
            return Err(error_invalid_input("cannot write after finishing"));
        }

        let mut total_written = 0;
        let mut remaining_buf = buf;

        while !remaining_buf.is_empty() {
            let block_remaining = self.block_size.saturating_sub(self.current_work_unit.len());
            let to_write = remaining_buf.len().min(block_remaining);

            if to_write > 0 {
                self.current_work_unit
                    .extend_from_slice(&remaining_buf[..to_write]);
                total_written += to_write;
                remaining_buf = &remaining_buf[to_write..];
            }

            if self.current_work_unit.len() >= self.block_size {
                self.send_work_unit()?;
            }

            while let Some(result) = self.get_next_compressed_chunk(false)? {
                self.write_compressed_block(
                    result.compressed_data,
                    result.checksum,
                    result.uncompressed_size,
                )?;
            }
        }

        Ok(total_written)
    }

    fn flush(&mut self) -> Result<()> {
        if !self.current_work_unit.is_empty() {
            self.send_work_unit()?;
        }

        let sequence_to_wait = self.next_sequence_to_dispatch;

        while self.next_sequence_to_write < sequence_to_wait {
            match self.get_next_compressed_chunk(true)? {
                Some(result) => {
                    self.write_compressed_block(
                        result.compressed_data,
                        result.checksum,
                        result.uncompressed_size,
                    )?;
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "compression stream ended unexpectedly during flush",
                    ));
                }
            }
        }

        self.inner.as_mut().expect("inner is empty").flush()?;
        Ok(())
    }
}

impl<W: Write> Drop for XZWriterMT<W> {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.work_queue.close();
        // Worker threads will exit when the work queue is closed.
        // JoinHandles will be dropped, which is fine since we set the shutdown flag.
    }
}
