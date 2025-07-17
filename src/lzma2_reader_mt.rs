use crate::LZMA2Reader;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::collections::BTreeMap;
use std::io;
use std::io::{Cursor, Read};
use std::num::NonZeroU8;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// A work unit for a worker thread.
/// Contains the sequence number and the raw compressed bytes for a series of chunks.
type WorkUnit = (u64, Vec<u8>);

/// A result unit from a worker thread.
/// Contains the sequence number and the decompressed data or an error.
type ResultUnit = (u64, io::Result<Vec<u8>>);

/// A multi-threaded LZMA2 decompressor.
///
/// # Examples
/// ```
/// use std::io::Read;
/// use std::num::NonZeroU8;
///
/// use lzma_rust2::{LZMA2Options, LZMA2ReaderMT};
///
/// let compressed: Vec<u8> = vec![
///     1, 0, 12, 72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33, 0,
/// ];
/// let mut reader = LZMA2ReaderMT::new(
///     compressed.as_slice(),
///     LZMA2Options::DICT_SIZE_DEFAULT,
///     None,
///     NonZeroU8::new(1).unwrap()
/// );
/// let mut decompressed = Vec::new();
/// reader.read_to_end(&mut decompressed).unwrap();
/// assert_eq!(&decompressed[..], b"Hello, world!");
/// ```
pub struct LZMA2ReaderMT {
    reader_handle: Option<thread::JoinHandle<()>>,
    result_rx: Receiver<ResultUnit>,
    next_sequence: u64,
    out_of_order_chunks: BTreeMap<u64, io::Result<Vec<u8>>>,
    current_chunk: Cursor<Vec<u8>>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
}

impl LZMA2ReaderMT {
    /// Creates a new multi-threaded LZMA2 reader.
    ///
    /// This function reads the initial LZMA properties byte from the stream
    /// before spawning the worker threads.
    ///
    /// - `inner`: The reader to read compressed data from.
    /// - `dict_size`: The dictionary size in bytes, as specified in the stream properties.
    /// - `preset_dict`: An optional preset dictionary.
    /// - `num_workers`: The number of worker threads to spawn for decompression.
    pub fn new<R: Read + Send + 'static>(
        inner: R,
        dict_size: u32,
        preset_dict: Option<&[u8]>,
        num_workers: NonZeroU8,
    ) -> Self {
        let num_workers = num_workers.get() as usize;
        let (work_tx, work_rx) = bounded::<WorkUnit>(num_workers + 1);
        let (result_tx, result_rx) = unbounded::<ResultUnit>();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let error_store = Arc::new(Mutex::new(None));

        // Spawn Reader Thread
        let reader_handle = {
            let shutdown_flag = Arc::clone(&shutdown_flag);
            let error_store = Arc::clone(&error_store);
            thread::spawn(move || {
                reader_thread_logic(inner, work_tx, shutdown_flag, error_store);
            })
        };

        // Spawn Worker Threads
        for _ in 0..num_workers {
            let work_rx = work_rx.clone();
            let result_tx = result_tx.clone();
            let shutdown_flag = Arc::clone(&shutdown_flag);
            let error_store = Arc::clone(&error_store);
            let preset_dict = preset_dict.map(|s| s.to_vec()).map(Arc::new);

            thread::spawn(move || {
                worker_thread_logic(
                    work_rx,
                    result_tx,
                    dict_size,
                    preset_dict,
                    shutdown_flag,
                    error_store,
                );
            });
        }

        Self {
            reader_handle: Some(reader_handle),
            result_rx,
            next_sequence: 0,
            out_of_order_chunks: BTreeMap::new(),
            current_chunk: Cursor::new(Vec::new()),
            shutdown_flag,
            error_store,
        }
    }

    /// Pulls the next available chunk, either from the out-of-order buffer
    /// or by waiting on the results channel.
    fn get_next_chunk(&mut self) -> io::Result<Vec<u8>> {
        // First, check if the chunk we need is already in the out-of-order buffer.
        if let Some(result) = self.out_of_order_chunks.remove(&self.next_sequence) {
            self.next_sequence += 1;
            return result;
        }

        // If not, we need to wait for results from the workers.
        loop {
            // Before blocking, check for a stored error.
            if let Some(err) = self.error_store.lock().unwrap().take() {
                return Err(err);
            }

            if self.shutdown_flag.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Decompression was cancelled due to an error in another thread.",
                ));
            }

            match self.result_rx.recv() {
                Ok((seq, result)) => {
                    if seq == self.next_sequence {
                        // This is the chunk we were waiting for.
                        self.next_sequence += 1;
                        return result;
                    } else {
                        // This is a future chunk, store it.
                        self.out_of_order_chunks.insert(seq, result);
                    }
                }
                Err(_) => {
                    // Channel is empty and all senders are gone. This means all
                    // workers have finished. Check the error store one last time.
                    if let Some(err) = self.error_store.lock().unwrap().take() {
                        return Err(err);
                    }
                    // No more chunks will ever arrive.
                    return Ok(Vec::new());
                }
            }
        }
    }
}

fn reader_thread_logic<R: Read>(
    mut inner: R,
    work_tx: Sender<WorkUnit>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
) {
    let mut sequence_id = 0;
    let mut current_work_unit = Vec::with_capacity(1024 * 1024);

    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            break;
        }

        let mut control_buf = [0u8; 1];
        match inner.read_exact(&mut control_buf) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                // Clean end of stream.
                break;
            }
            Err(error) => {
                set_error(error, &error_store, &shutdown_flag);
                break;
            }
        }
        let control = control_buf[0];

        // End of stream marker. Add it to the current unit and finish.
        if control == 0x00 {
            current_work_unit.push(control);
            break;
        }

        // A dictionary reset marks the beginning of a new, independent work unit.
        // The single-threaded reader requires that any new stream it processes
        // starts with a chunk that resets the dictionary.
        let is_dict_reset = control >= 0xE0 || control == 0x01;

        // If this chunk is a dictionary reset AND we have already buffered
        // some data, it means the previous work unit is complete. Send it.
        if is_dict_reset && !current_work_unit.is_empty() {
            let work_to_send = std::mem::take(&mut current_work_unit);
            if work_tx.send((sequence_id, work_to_send)).is_err() {
                // Main thread is gone, shut down.
                return;
            }
            sequence_id += 1;
        }

        // Now, add the current chunk (which is either the first chunk of the
        // first work unit, a new dictionary-reset chunk for a new unit, or a
        // subsequent chunk for the current unit) to the buffer.
        current_work_unit.push(control);

        // This parsing logic for the rest of the chunk is correct.
        let chunk_size_to_read = if control >= 0x80 {
            // LZMA chunk
            let mut header_buf = [0u8; 4];
            if let Err(e) = inner.read_exact(&mut header_buf) {
                set_error(e, &error_store, &shutdown_flag);
                break;
            }
            current_work_unit.extend_from_slice(&header_buf);

            if control >= 0xC0 {
                let mut props_buf = [0u8; 1];
                if let Err(e) = inner.read_exact(&mut props_buf) {
                    set_error(e, &error_store, &shutdown_flag);
                    break;
                }
                current_work_unit.push(props_buf[0]);
            }
            u16::from_be_bytes([header_buf[2], header_buf[3]]) as usize + 1
        } else if control == 0x02 {
            // Uncompressed chunk, no reset.
            let mut size_buf = [0u8; 2];
            if let Err(e) = inner.read_exact(&mut size_buf) {
                set_error(e, &error_store, &shutdown_flag);
                break;
            }
            current_work_unit.extend_from_slice(&size_buf);
            u16::from_be_bytes(size_buf) as usize + 1
        } else if control == 0x01 {
            // Uncompressed chunk with dict reset.
            0
        } else {
            set_error(
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid LZMA2 control byte: {}", control),
                ),
                &error_store,
                &shutdown_flag,
            );
            break;
        };

        // Read the chunk data itself.
        if chunk_size_to_read > 0 {
            let start_len = current_work_unit.len();
            current_work_unit.resize(start_len + chunk_size_to_read, 0);
            if let Err(e) = inner.read_exact(&mut current_work_unit[start_len..]) {
                set_error(e, &error_store, &shutdown_flag);
                break;
            }
        }
    }

    // After the loop, send any remaining data as the final work unit.
    if !current_work_unit.is_empty() {
        let _ = work_tx.send((sequence_id, current_work_unit));
    }
}

/// The logic for a single worker thread.
fn worker_thread_logic(
    work_rx: Receiver<WorkUnit>,
    result_tx: Sender<ResultUnit>,
    dict_size: u32,
    preset_dict: Option<Arc<Vec<u8>>>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
) {
    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            break;
        }

        let work_unit = work_rx.recv();

        match work_unit {
            Ok((seq, compressed_data)) => {
                let mut reader = LZMA2Reader::new(
                    compressed_data.as_slice(),
                    dict_size,
                    preset_dict.as_deref().map(|v| v.as_slice()),
                );

                let mut decompressed_data = Vec::with_capacity(compressed_data.len());
                let result = match reader.read_to_end(&mut decompressed_data) {
                    Ok(_) => Ok(decompressed_data),
                    Err(error) => {
                        set_error(error, &error_store, &shutdown_flag);
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Decompression failed in worker thread.",
                        ))
                    }
                };

                if result_tx.send((seq, result)).is_err() {
                    // If the receiver is gone, we can just shut down.
                    break;
                }
            }
            Err(_) => {
                // The channel is empty and the sender (reader thread) is gone.
                // This is the signal for the worker to terminate.
                break;
            }
        }
    }
}

/// Helper to set the shared error state and trigger shutdown.
fn set_error(
    error: io::Error,
    error_store: &Arc<Mutex<Option<io::Error>>>,
    shutdown_flag: &Arc<AtomicBool>,
) {
    let mut guard = error_store.lock().unwrap();
    if guard.is_none() {
        *guard = Some(error);
    }
    shutdown_flag.store(true, Ordering::Relaxed);
}

impl Read for LZMA2ReaderMT {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // Try reading from the current chunk buffer first.
        let bytes_read = self.current_chunk.read(buf)?;
        if bytes_read > 0 {
            return Ok(bytes_read);
        }

        // If the buffer is empty, we need to get the next decompressed chunk.
        let chunk_data = self.get_next_chunk()?;

        if chunk_data.is_empty() {
            // This is the clean end of the stream.
            Ok(0)
        } else {
            self.current_chunk = Cursor::new(chunk_data);
            // Recursively call read to fill the buffer from the new chunk.
            self.read(buf)
        }
    }
}

impl Drop for LZMA2ReaderMT {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);

        // The worker threads will exit automatically when the channels close.
        // We need to explicitly join the reader thread to ensure it has finished
        // with the `inner` reader before it might be dropped.
        if let Some(handle) = self.reader_handle.take() {
            // It's possible the reader thread is blocked sending to a full work_tx.
            // To wake it up, we can drain the result channel, which might unblock workers,
            // which in turn unblocks the work channel.
            let _ = handle.join();
        }
    }
}
