use std::{
    collections::BTreeMap,
    io::{self, Cursor, Seek, SeekFrom},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
};

use super::{HEADER_SIZE, TRAILER_SIZE};
use crate::{
    error_invalid_data, set_error,
    work_queue::{WorkStealingQueue, WorkerHandle},
    LZIPReader, Read,
};

#[derive(Debug, Clone)]
struct LZIPMember {
    start_pos: u64,
    compressed_size: u64,
}

/// A work unit for a worker thread.
/// Contains the sequence number and the raw compressed bytes for a single LZIP member.
type WorkUnit = (u64, Vec<u8>);

/// A result unit from a worker thread.
/// Contains the sequence number and the decompressed data.
type ResultUnit = (u64, Vec<u8>);

enum State {
    /// Dispatching members to worker threads.
    Dispatching,
    /// All members dispatched, waiting for workers to complete.
    Draining,
    /// All data has been decompressed and returned. The stream is exhausted.
    Finished,
    /// A fatal error occurred in either the reader or a worker thread.
    Error,
}

/// A multi-threaded LZIP decompressor.
pub struct LZIPReaderMT<R: Read + Seek> {
    inner: Option<R>,
    members: Vec<LZIPMember>,
    result_rx: Receiver<ResultUnit>,
    result_tx: Sender<ResultUnit>,
    next_sequence_to_dispatch: u64,
    next_sequence_to_return: u64,
    last_sequence_id: Option<u64>,
    out_of_order_chunks: BTreeMap<u64, Vec<u8>>,
    current_chunk: Cursor<Vec<u8>>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    state: State,
    work_queue: WorkStealingQueue<WorkUnit>,
    active_workers: Arc<AtomicU32>,
    max_workers: u32,
    worker_handles: Vec<thread::JoinHandle<()>>,
}

impl<R: Read + Seek> LZIPReaderMT<R> {
    /// Creates a new multi-threaded LZIP reader.
    ///
    /// - `inner`: The reader to read compressed data from. Must implement Seek.
    /// - `num_workers`: The maximum number of worker threads for decompression. Currently capped at 256 threads.
    pub fn new(inner: R, num_workers: u32) -> io::Result<Self> {
        let max_workers = num_workers.clamp(1, 256);

        let work_queue = WorkStealingQueue::new();
        let (result_tx, result_rx) = mpsc::channel::<ResultUnit>();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let error_store = Arc::new(Mutex::new(None));
        let active_workers = Arc::new(AtomicU32::new(0));

        let mut reader = Self {
            inner: Some(inner),
            members: Vec::new(),
            result_rx,
            result_tx,
            next_sequence_to_dispatch: 0,
            next_sequence_to_return: 0,
            last_sequence_id: None,
            out_of_order_chunks: BTreeMap::new(),
            current_chunk: Cursor::new(Vec::new()),
            shutdown_flag,
            error_store,
            state: State::Dispatching,
            work_queue,
            active_workers,
            max_workers,
            worker_handles: Vec::new(),
        };

        reader.scan_members()?;

        Ok(reader)
    }

    /// Scan the LZIP file to collect information about all members.
    /// This reads from the back of the file to efficiently locate member boundaries.
    fn scan_members(&mut self) -> io::Result<()> {
        let mut reader = self.inner.take().expect("inner reader not set");

        let file_size = reader.seek(SeekFrom::End(0))?;

        if file_size < (HEADER_SIZE + TRAILER_SIZE) as u64 {
            self.inner = Some(reader);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File too small to contain a valid LZIP member",
            ));
        }

        let mut current_pos = file_size;

        while current_pos > 0 {
            if current_pos < TRAILER_SIZE as u64 {
                break;
            }

            // Seek to read the trailer (last 20 bytes of current member).
            reader.seek(SeekFrom::Start(current_pos - TRAILER_SIZE as u64))?;
            let mut trailer_buf = [0u8; TRAILER_SIZE];
            reader.read_exact(&mut trailer_buf)?;

            // member_size is in bytes 12-19 of the trailer (little endian)
            let member_size = u64::from_le_bytes([
                trailer_buf[12],
                trailer_buf[13],
                trailer_buf[14],
                trailer_buf[15],
                trailer_buf[16],
                trailer_buf[17],
                trailer_buf[18],
                trailer_buf[19],
            ]);

            if member_size == 0 || member_size > current_pos {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid LZIP member size in trailer",
                ));
            }

            let member_start = current_pos - member_size;

            // Verify this looks like a valid LZIP header.
            reader.seek(SeekFrom::Start(member_start))?;
            let mut header_buf = [0u8; 4];
            reader.read_exact(&mut header_buf)?;

            if header_buf != [b'L', b'Z', b'I', b'P'] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid LZIP magic bytes",
                ));
            }

            self.members.push(LZIPMember {
                start_pos: member_start,
                compressed_size: member_size,
            });

            current_pos = member_start;
        }

        if self.members.is_empty() {
            self.inner = Some(reader);
            return Err(error_invalid_data("No valid LZIP members found"));
        }

        // Reverse to get members in forward order.
        self.members.reverse();
        self.inner = Some(reader);

        Ok(())
    }

    fn spawn_worker_thread(&mut self) {
        let worker_handle = self.work_queue.worker();
        let result_tx = self.result_tx.clone();
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let error_store = Arc::clone(&self.error_store);
        let active_workers = Arc::clone(&self.active_workers);

        let handle = thread::spawn(move || {
            worker_thread_logic(
                worker_handle,
                result_tx,
                shutdown_flag,
                error_store,
                active_workers,
            );
        });

        self.worker_handles.push(handle);
    }

    /// Get the count of LZIP members found in the file.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    fn dispatch_next_member(&mut self) -> io::Result<bool> {
        let member_index = self.next_sequence_to_dispatch as usize;

        if member_index >= self.members.len() {
            // No more members to dispatch.
            return Ok(false);
        }

        let member = &self.members[member_index];
        let mut reader = self.inner.take().expect("inner reader not set");

        reader.seek(SeekFrom::Start(member.start_pos))?;
        let mut member_data = vec![0u8; member.compressed_size as usize];
        reader.read_exact(&mut member_data)?;

        self.inner = Some(reader);

        if !self
            .work_queue
            .push((self.next_sequence_to_dispatch, member_data))
        {
            // Queue is closed, this indicates shutdown.
            self.state = State::Error;
            set_error(
                io::Error::new(io::ErrorKind::BrokenPipe, "Worker threads have shut down"),
                &self.error_store,
                &self.shutdown_flag,
            );
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Worker threads have shut down",
            ));
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
        Ok(true)
    }

    fn get_next_uncompressed_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            // Always check for already-received chunks first.
            if let Some(result) = self
                .out_of_order_chunks
                .remove(&self.next_sequence_to_return)
            {
                self.next_sequence_to_return += 1;
                return Ok(Some(result));
            }

            // Check for a globally stored error.
            if let Some(err) = self.error_store.lock().unwrap().take() {
                self.state = State::Error;
                return Err(err);
            }

            match self.state {
                State::Dispatching => {
                    // First, always try to receive a result without blocking.
                    // This keeps the pipeline moving and avoids unnecessary blocking.
                    match self.result_rx.try_recv() {
                        Ok((seq, result)) => {
                            if seq == self.next_sequence_to_return {
                                self.next_sequence_to_return += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(seq, result);
                                continue; // Loop again to check the out_of_order_chunks
                            }
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            // All workers are done.
                            self.state = State::Draining;
                            continue;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            // No results are ready. Now, we can consider dispatching more work.
                        }
                    }

                    // If the work queue has capacity, try to read more from the source.
                    if self.work_queue.len() < 4 {
                        match self.dispatch_next_member() {
                            Ok(true) => {
                                // Successfully read and dispatched a chunk, loop to continue.
                                continue;
                            }
                            Ok(false) => {
                                // No more members to dispatch.
                                // Set the last sequence ID and transition to draining.
                                self.last_sequence_id =
                                    Some(self.next_sequence_to_dispatch.saturating_sub(1));
                                self.state = State::Draining;
                                continue;
                            }
                            Err(error) => {
                                set_error(error, &self.error_store, &self.shutdown_flag);
                                self.state = State::Error;
                                continue;
                            }
                        }
                    }

                    // Now we MUST wait for a result to make progress.
                    match self.result_rx.recv() {
                        Ok((seq, result)) => {
                            if seq == self.next_sequence_to_return {
                                self.next_sequence_to_return += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(seq, result);
                                // We've made progress, loop to check the out_of_order_chunks
                                continue;
                            }
                        }
                        Err(_) => {
                            // All workers are done.
                            self.state = State::Draining;
                        }
                    }
                }
                State::Draining => {
                    if let Some(last_seq) = self.last_sequence_id {
                        if self.next_sequence_to_return > last_seq {
                            self.state = State::Finished;
                            continue;
                        }
                    }

                    // In Draining state, we only wait for results.
                    match self.result_rx.recv() {
                        Ok((seq, result)) => {
                            if seq == self.next_sequence_to_return {
                                self.next_sequence_to_return += 1;
                                return Ok(Some(result));
                            } else {
                                self.out_of_order_chunks.insert(seq, result);
                            }
                        }
                        Err(_) => {
                            // All workers finished, and channel is empty. We are done.
                            self.state = State::Finished;
                        }
                    }
                }
                State::Finished => {
                    return Ok(None);
                }
                State::Error => {
                    // The error was already logged, now we just propagate it.
                    return Err(self.error_store.lock().unwrap().take().unwrap_or_else(|| {
                        io::Error::other("decompression failed with an unknown error")
                    }));
                }
            }
        }
    }
}

/// The logic for a single worker thread.
fn worker_thread_logic(
    worker_handle: WorkerHandle<WorkUnit>,
    result_tx: Sender<ResultUnit>,
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

        let reader_result = LZIPReader::new(work_unit_data.as_slice());

        let mut lzip_reader = match reader_result {
            Ok(reader) => reader,
            Err(error) => {
                active_workers.fetch_sub(1, Ordering::Release);
                set_error(error, &error_store, &shutdown_flag);
                return;
            }
        };

        let mut decompressed_data = Vec::new();
        let result = match lzip_reader.read_to_end(&mut decompressed_data) {
            Ok(_) => decompressed_data,
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

impl<R: Read + Seek> Read for LZIPReaderMT<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let bytes_read = self.current_chunk.read(buf)?;

        if bytes_read > 0 {
            return Ok(bytes_read);
        }

        let chunk_data = self.get_next_uncompressed_chunk()?;

        let Some(chunk_data) = chunk_data else {
            // This is the clean end of the stream.
            return Ok(0);
        };

        self.current_chunk = Cursor::new(chunk_data);

        // Recursive call to read the new chunk data.
        self.read(buf)
    }
}

impl<R: Read + Seek> Drop for LZIPReaderMT<R> {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.work_queue.close();
        // Worker threads will exit when the work queue is closed.
        // JoinHandles will be dropped, which is fine since we set the shutdown flag.
    }
}
