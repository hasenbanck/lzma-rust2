#[cfg(target_has_atomic = "64")]
use std::sync::atomic::AtomicU64;
use std::{
    io::{self, Cursor, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::SyncSender,
    },
};

use super::{LzipOptions, LzipWriter};
use crate::{
    AutoFinish, AutoFinisher, error_invalid_input, set_error,
    work_pool::{WorkPool, WorkPoolConfig},
    work_queue::WorkerHandle,
};

/// A work unit for a worker thread.
#[derive(Debug, Clone)]
struct WorkUnit {
    data: Vec<u8>,
    options: LzipOptions,
    #[cfg(target_has_atomic = "64")]
    progress: Option<Arc<AtomicU64>>,
}

/// A multi-threaded LZIP compressor.
pub struct LzipWriterMt<W: Write> {
    inner: W,
    options: LzipOptions,
    current_work_unit: Vec<u8>,
    member_size: usize,
    work_pool: WorkPool<WorkUnit, Vec<u8>>,
    current_chunk: Cursor<Vec<u8>>,
    pending_write_data: Vec<u8>,
    #[cfg(target_has_atomic = "64")]
    progress: Option<Arc<AtomicU64>>,
}

impl<W: Write> LzipWriterMt<W> {
    /// Creates a new multi-threaded LZIP writer.
    ///
    /// - `inner`: The writer to write compressed data to.
    /// - `options`: The LZIP options used for compressing. Member size must be set when using the
    ///   multi-threaded encoder. If you need just one member, then use the single-threaded encoder.
    /// - `num_workers`: The maximum number of worker threads for compression.
    ///   Currently capped at 256 threads.
    pub fn new(inner: W, options: LzipOptions, num_workers: u32) -> io::Result<Self> {
        let member_size = match options.member_size {
            None => return Err(error_invalid_input("member size must be set")),
            Some(member_size) => member_size.get().max(options.lzma_options.dict_size as u64),
        };

        let member_size = usize::try_from(member_size)
            .map_err(|_| error_invalid_input("member size bigger than usize"))?;

        // We don't know how many work units we'll have ahead of time.
        let num_work = u64::MAX;

        Ok(Self {
            inner,
            options,
            current_work_unit: Vec::with_capacity(member_size.min(1024 * 1024)),
            member_size,
            work_pool: WorkPool::new(
                WorkPoolConfig::new(num_workers, num_work),
                worker_thread_logic,
            ),
            current_chunk: Cursor::new(Vec::new()),
            pending_write_data: Vec::new(),
            #[cfg(target_has_atomic = "64")]
            progress: None,
        })
    }

    /// Sets a shared counter for the uncompressed bytes passed to the encoder.
    ///
    /// The writer only adds to the counter. It equals the total input after a successful `finish`.
    ///
    /// The counter should be replaced before writing input. Calling this method after
    /// writing input returns [`io::ErrorKind::InvalidInput`] and keeps the existing counter.
    #[cfg(target_has_atomic = "64")]
    pub fn set_progress(&mut self, counter: Arc<AtomicU64>) -> io::Result<()> {
        if !self.current_work_unit.is_empty() || self.work_pool.next_index_to_dispatch() != 0 {
            return Err(error_invalid_input(
                "progress must be configured before writing input",
            ));
        }
        self.progress = Some(counter);
        Ok(())
    }

    /// Sends the current work unit to the workers, writing any available results.
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

        let work_data = core::mem::take(&mut self.current_work_unit);
        let mut single_member_options = self.options.clone();
        single_member_options.member_size = None;

        let mut work_data_opt = Some(work_data);

        self.work_pool.dispatch_next_work(&mut |_seq| {
            let data = work_data_opt.take().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "work already provided")
            })?;
            Ok(WorkUnit {
                data,
                options: single_member_options.clone(),
                #[cfg(target_has_atomic = "64")]
                progress: self.progress.clone(),
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
            let remaining = self
                .member_size
                .saturating_sub(self.current_work_unit.len());
            let to_write = remaining_buf.len().min(remaining);
            self.current_work_unit
                .extend_from_slice(&remaining_buf[..to_write]);
            remaining_buf = &remaining_buf[to_write..];

            if self.current_work_unit.len() >= self.member_size {
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

    /// Consume the LzipWriterMt and return the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Finishes the compression and returns the underlying writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.work_pool.check_error_and_abort()?;
        if !self.current_work_unit.is_empty() {
            self.send_work_unit()?;
        }

        // If no data was provided to compress, write an empty LZIP file (single empty member).
        if self.work_pool.next_index_to_dispatch() == 0 {
            let mut options = self.options.clone();
            options.member_size = None;
            let lzip_writer = LzipWriter::new(Vec::new(), options);
            let empty_member = lzip_writer.finish()?;

            self.inner.write_all(&empty_member)?;
            self.inner.flush()?;

            return Ok(self.inner);
        }

        // Mark the WorkPool as finished so it knows no more work is coming.
        self.work_pool.finish();

        // Wait for all remaining work to complete.
        while let Some(compressed_data) = self.work_pool.get_dispatched_result()? {
            self.inner.write_all(&compressed_data)?;
        }

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

        let mut compressed_buffer = Vec::new();

        let mut writer = LzipWriter::new(&mut compressed_buffer, work_unit.options);
        // Feed the encoder 64 KiB at a time, like `Lzma2WriterMt`, so that progress rises
        // while the work unit is encoded. The compressed output is the same as a single write.
        let mut chunks = work_unit.data.chunks(64 * 1024);
        let write_result = chunks.try_for_each(|chunk| -> io::Result<()> {
            writer.write_all(chunk)?;
            #[cfg(target_has_atomic = "64")]
            if let Some(progress) = &work_unit.progress {
                progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            Ok(())
        });
        let result = match write_result {
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

        if result_tx.send((index, result)).is_err() {
            active_workers.fetch_sub(1, Ordering::Release);
            return;
        }

        active_workers.fetch_sub(1, Ordering::Release);
    }
}

impl<W: Write> Write for LzipWriterMt<W> {
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

impl<W: Write> AutoFinish for LzipWriterMt<W> {
    fn finish_ignore_error(self) {
        let _ = self.finish();
    }
}
