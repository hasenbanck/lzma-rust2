use std::{
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::SyncSender,
    },
};

use super::Lzma2Writer;
use crate::{
    AutoFinish, AutoFinisher, ByteWriter, EncoderCancelled, Lzma2Options, error_invalid_input,
    set_error,
    work_pool::{WorkPool, WorkPoolConfig},
    work_queue::WorkerHandle,
};

/// A work unit for a worker thread.
#[derive(Debug, Clone)]
struct WorkUnit {
    data: Vec<u8>,
    options: Lzma2Options,
    cancellation: Option<Arc<AtomicBool>>,
}

/// A multi-threaded LZMA2 compressor.
pub struct Lzma2WriterMt<W: Write> {
    inner: W,
    options: Lzma2Options,
    chunk_size: usize,
    current_work_unit: Vec<u8>,
    work_pool: WorkPool<WorkUnit, Vec<u8>>,
    cancellation: Option<Arc<AtomicBool>>,
}

impl<W: Write> Lzma2WriterMt<W> {
    /// Creates a new multi-threaded LZMA2 writer.
    ///
    /// - `inner`: The writer to write compressed data to.
    /// - `options`: The LZMA2 options used for compressing. Chunk size must be set when using the
    ///   multi-threaded encoder. If you need just one chunk, then use the single-threaded encoder.
    /// - `num_workers`: The maximum number of worker threads for compression.
    ///   Currently capped at 256 Threads.
    pub fn new(inner: W, options: Lzma2Options, num_workers: u32) -> crate::Result<Self> {
        let chunk_size = match options.chunk_size {
            None => return Err(error_invalid_input("chunk size must be set")),
            Some(chunk_size) => chunk_size.get().max(options.lzma_options.dict_size as u64),
        };

        let chunk_size = usize::try_from(chunk_size)
            .map_err(|_| error_invalid_input("chunk size bigger than usize"))?;

        // We don't know how many work units we'll have ahead of time.
        let num_work = u64::MAX;

        Ok(Self {
            inner,
            options,
            chunk_size,
            current_work_unit: Vec::with_capacity(chunk_size),
            work_pool: WorkPool::new(
                WorkPoolConfig::new(num_workers, num_work),
                worker_thread_logic,
            ),
            cancellation: None,
        })
    }

    /// Sets a shared cancellation flag before writing input.
    ///
    /// The flag can be replaced before writing input. Calling this method after
    /// writing input returns [`io::ErrorKind::InvalidInput`] and keeps the existing flag.
    ///
    /// Setting the flag to `true` requests cancellation. Cancellation returns an I/O
    /// error carrying [`EncoderCancelled`] and stops the writer permanently.
    pub fn set_cancellation(&mut self, flag: Arc<AtomicBool>) -> io::Result<()> {
        if !self.current_work_unit.is_empty() || self.work_pool.next_index_to_dispatch() != 0 {
            return Err(error_invalid_input(
                "cancellation must be configured before writing input",
            ));
        }
        self.work_pool.set_cancellation(Arc::clone(&flag));
        self.cancellation = Some(flag);
        Ok(())
    }

    /// Sends the current work unit to the workers.
    fn send_work_unit(&mut self) -> io::Result<()> {
        if self.current_work_unit.is_empty() {
            return Ok(());
        }

        self.drain_available_results()?;

        while self.work_pool.is_full() {
            if let Some(compressed_data) = self.work_pool.get_dispatched_result()? {
                self.inner.write_all(&compressed_data)?;
            }
        }

        let work_data = core::mem::replace(
            &mut self.current_work_unit,
            Vec::with_capacity(self.chunk_size),
        );
        let mut single_chunk_options = self.options.clone();
        single_chunk_options.chunk_size = None;
        single_chunk_options.lzma_options.preset_dict = None;

        let mut work_data_opt = Some(work_data);

        self.work_pool.dispatch_next_work(&mut |_seq| {
            let data = work_data_opt.take().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "work already provided")
            })?;
            Ok(WorkUnit {
                data,
                options: single_chunk_options.clone(),
                cancellation: self.cancellation.clone(),
            })
        })?;

        self.drain_available_results()?;

        Ok(())
    }

    /// Drains all currently available results from the work pool and writes them.
    fn drain_available_results(&mut self) -> io::Result<()> {
        while let Some(compressed_data) = self.work_pool.try_get_result()? {
            self.inner.write_all(&compressed_data)?;
        }
        Ok(())
    }

    fn write_inner(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.work_pool.check_error_and_abort()?;

        let total_written = buf.len();
        let mut remaining_buf = buf;

        while !remaining_buf.is_empty() {
            let remaining = self.chunk_size.saturating_sub(self.current_work_unit.len());
            let to_write = remaining_buf.len().min(remaining);
            self.current_work_unit
                .extend_from_slice(&remaining_buf[..to_write]);
            remaining_buf = &remaining_buf[to_write..];

            if self.current_work_unit.len() >= self.chunk_size {
                self.send_work_unit()?;
            }

            self.drain_available_results()?;
        }

        Ok(total_written)
    }

    fn flush_inner(&mut self) -> io::Result<()> {
        self.work_pool.check_error_and_abort()?;

        if !self.current_work_unit.is_empty() {
            self.send_work_unit()?;
        }

        // Wait for all pending work to complete and write the results.
        while let Some(compressed_data) = self.work_pool.get_dispatched_result()? {
            self.inner.write_all(&compressed_data)?;
        }

        self.inner.flush()
    }

    /// Returns a wrapper around `self` that will finish the stream on drop.
    pub fn auto_finish(self) -> AutoFinisher<Self> {
        AutoFinisher(Some(self))
    }

    /// Consume the Lzma2WriterMt and return the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Finishes the compression and returns the underlying writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.work_pool.check_error_and_abort()?;
        if !self.current_work_unit.is_empty() {
            self.send_work_unit()?;
        }

        // If no data was provided to compress, write an empty LZMA2 stream.
        if self.work_pool.next_index_to_dispatch() == 0 {
            self.inner.write_u8(0x00)?;
            self.inner.flush()?;

            return Ok(self.inner);
        }

        // Mark the WorkPool as finished so it knows no more work is coming.
        self.work_pool.finish();

        // Wait for all remaining work to complete.
        while let Some(compressed_data) = self.work_pool.get_dispatched_result()? {
            self.inner.write_all(&compressed_data)?;
        }

        self.inner.write_u8(0x00)?;
        self.inner.flush()?;

        Ok(self.inner)
    }
}

/// The logic for a single worker thread.
fn worker_thread_logic(
    worker_handle: WorkerHandle<(u64, WorkUnit)>,
    result_tx: SyncSender<(u64, Vec<u8>)>,
    shutdown_flag: Arc<AtomicBool>,
    error_store: Arc<Mutex<Option<io::Error>>>,
    active_workers: Arc<AtomicU32>,
) {
    while !shutdown_flag.load(Ordering::Acquire) {
        let (index, work_unit) = match worker_handle.steal() {
            Some(work) => {
                active_workers.fetch_add(1, Ordering::Release);
                work
            }
            None => {
                // No more work available and queue is closed.
                break;
            }
        };

        let result = match encode_work_unit(work_unit, &shutdown_flag, Vec::new()) {
            Ok(Some(result)) => result,
            Ok(None) => {
                active_workers.fetch_sub(1, Ordering::Release);
                return;
            }
            Err(error) => {
                active_workers.fetch_sub(1, Ordering::Release);
                set_error(error, &error_store, &shutdown_flag);
                return;
            }
        };

        if result_tx.send((index, result)).is_err() {
            active_workers.fetch_sub(1, Ordering::Release);
            return;
        }

        active_workers.fetch_sub(1, Ordering::Release);
    }
}

/// Encode a work unit, returning `None` when the pool shuts down.
/// Requested cancellation returns an error carrying `EncoderCancelled`.
fn encode_work_unit<W: Write>(
    work_unit: WorkUnit,
    shutdown_flag: &AtomicBool,
    inner: W,
) -> io::Result<Option<W>> {
    let WorkUnit {
        data,
        options,
        cancellation,
    } = work_unit;

    let should_continue = || {
        if shutdown_flag.load(Ordering::Acquire) {
            return Ok(false);
        }
        if cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(io::Error::other(EncoderCancelled));
        }
        Ok(true)
    };
    if !should_continue()? {
        return Ok(None);
    }
    let mut writer = Lzma2Writer::new(inner, options);
    for chunk in data.chunks(64 * 1024) {
        if !should_continue()? {
            return Ok(None);
        }
        writer.write_all(chunk)?;
    }
    if !writer.flush_with_check(should_continue)? {
        return Ok(None);
    }
    Ok(Some(writer.into_inner()))
}

impl<W: Write> Write for Lzma2WriterMt<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let result = self.write_inner(buf);
        if result.is_err() {
            self.work_pool.abort();
        }
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.flush_inner();
        if result.is_err() {
            self.work_pool.abort();
        }
        result
    }
}

impl<W: Write> AutoFinish for Lzma2WriterMt<W> {
    fn finish_ignore_error(self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{atomic::AtomicUsize, mpsc},
        thread,
        time::Duration,
    };

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(5);

    struct GatedSink {
        gate: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
        bytes_written: Arc<AtomicUsize>,
    }

    impl Write for GatedSink {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            if let Some((started, resume)) = self.gate.take() {
                started.send(()).unwrap();
                resume.recv_timeout(DEADLINE).unwrap();
            }
            self.bytes_written.fetch_add(input.len(), Ordering::Relaxed);
            Ok(input.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn cancellation_stops_active_worker_encoding() {
        let flag = Arc::new(AtomicBool::new(false));
        let cancellation = Arc::clone(&flag);
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let bytes_written = Arc::new(AtomicUsize::new(0));
        let sink = GatedSink {
            gate: Some((started_tx, resume_rx)),
            bytes_written: Arc::clone(&bytes_written),
        };
        let worker = thread::spawn(move || {
            encode_work_unit(
                work_unit(512 * 1024, Some(cancellation)),
                &AtomicBool::new(false),
                sink,
            )
        });

        started_rx.recv_timeout(DEADLINE).unwrap();
        flag.store(true, Ordering::Relaxed);
        resume_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        let error = result.err().expect("worker ignored cancellation");
        assert!(error.get_ref().unwrap().is::<EncoderCancelled>());
        assert!(bytes_written.load(Ordering::Relaxed) < 256 * 1024);
    }

    #[test]
    fn shutdown_stops_active_worker_without_reporting_cancellation() {
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::clone(&shutdown_flag);
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let bytes_written = Arc::new(AtomicUsize::new(0));
        let sink = GatedSink {
            gate: Some((started_tx, resume_rx)),
            bytes_written: Arc::clone(&bytes_written),
        };
        let worker =
            thread::spawn(move || encode_work_unit(work_unit(512 * 1024, None), &shutdown, sink));

        started_rx.recv_timeout(DEADLINE).unwrap();
        shutdown_flag.store(true, Ordering::Release);
        resume_tx.send(()).unwrap();
        assert!(worker.join().unwrap().unwrap().is_none());
        assert!(bytes_written.load(Ordering::Relaxed) < 256 * 1024);
    }

    #[test]
    fn cancellation_stops_worker_during_final_flush() {
        let flag = Arc::new(AtomicBool::new(false));
        let cancellation = Arc::clone(&flag);
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let sink = GatedSink {
            gate: Some((started_tx, resume_rx)),
            bytes_written: Arc::new(AtomicUsize::new(0)),
        };
        let worker = thread::spawn(move || {
            // This fits in the encoder's lookahead buffer, so output starts in flush().
            encode_work_unit(
                work_unit(128, Some(cancellation)),
                &AtomicBool::new(false),
                sink,
            )
        });

        started_rx.recv_timeout(DEADLINE).unwrap();
        flag.store(true, Ordering::Relaxed);
        resume_tx.send(()).unwrap();
        let error = worker
            .join()
            .unwrap()
            .err()
            .expect("worker ignored cancellation during flush");
        assert!(error.get_ref().unwrap().is::<EncoderCancelled>());
    }

    #[test]
    fn shutdown_stops_worker_during_final_flush_without_reporting_cancellation() {
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::clone(&shutdown_flag);
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let sink = GatedSink {
            gate: Some((started_tx, resume_rx)),
            bytes_written: Arc::new(AtomicUsize::new(0)),
        };
        let worker = thread::spawn(move || encode_work_unit(work_unit(128, None), &shutdown, sink));

        started_rx.recv_timeout(DEADLINE).unwrap();
        shutdown_flag.store(true, Ordering::Release);
        resume_tx.send(()).unwrap();
        assert!(worker.join().unwrap().unwrap().is_none());
    }

    fn work_unit(input_size: usize, cancellation: Option<Arc<AtomicBool>>) -> WorkUnit {
        let mut state = 0x1234_5678_u32;
        let data = (0..input_size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let mut options = Lzma2Options::with_preset(1);
        options.lzma_options.dict_size = 512 * 1024;
        WorkUnit {
            data,
            options,
            cancellation,
        }
    }

    #[test]
    fn dispatch_retains_a_reserved_producer_buffer() {
        let mut options = Lzma2Options::with_preset(5);
        options.lzma_options.dict_size = 64 * 1024;
        options.set_chunk_size(std::num::NonZeroU64::new(64 * 1024));
        let mut writer = Lzma2WriterMt::new(Vec::new(), options, 2).unwrap();
        writer.write_all(&vec![0; 64 * 1024]).unwrap();
        assert!(writer.current_work_unit.is_empty());
        assert!(writer.current_work_unit.capacity() >= writer.chunk_size);
        writer.finish().unwrap();
    }
}
