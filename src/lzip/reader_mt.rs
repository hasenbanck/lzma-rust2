use std::{
    io::{self, Cursor, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::SyncSender,
    },
};

use super::{HEADER_SIZE, LzipHeader, LzipMember, scan_members};
use crate::{
    LzipReader, Read, error_out_of_memory,
    lzma_reader::{get_memory_usage, speculation_memory_usage},
    set_error,
    work_pool::{WorkPool, WorkPoolConfig, WorkPoolState},
    work_queue::WorkerHandle,
};

/// A work unit for a worker thread.
#[derive(Debug)]
struct WorkUnit {
    member_data: Vec<u8>,
    output_limit: Option<usize>,
}

/// A multi-threaded LZIP decompressor.
pub struct LzipReaderMt<R: Read + Seek> {
    inner: R,
    members: Vec<LzipMember>,
    work_pool: WorkPool<WorkUnit, Vec<u8>>,
    current_chunk: Cursor<Vec<u8>>,
    mem_limit_kb: u32,
}

impl<R: Read + Seek> LzipReaderMt<R> {
    /// Creates a new multi-threaded LZIP reader.
    ///
    /// - `inner`: The reader to read compressed data from. Must implement Seek.
    /// - `num_workers`: The maximum number of worker threads for decompression. Currently capped at 256 threads.
    pub fn new(inner: R, num_workers: u32) -> io::Result<Self> {
        Self::new_mem_limit(inner, u32::MAX, num_workers)
    }

    /// Creates a multi-threaded LZIP reader with a per-member memory limit in KiB.
    /// `u32::MAX` means no limit. The limit covers the estimated decoder memory,
    /// compressed member buffer, and decompressed member buffer.
    pub fn new_mem_limit(inner: R, mem_limit_kb: u32, num_workers: u32) -> io::Result<Self> {
        let (inner, members) = scan_members(inner)?;
        let num_members = members.len() as u64;

        Ok(Self {
            inner,
            members,
            work_pool: WorkPool::new(
                WorkPoolConfig::new(num_workers, num_members),
                worker_thread_logic,
            ),
            current_chunk: Cursor::new(Vec::new()),
            mem_limit_kb,
        })
    }

    /// Get the count of LZIP members found in the file.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    fn get_next_uncompressed_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        // Check if we've processed all members
        if matches!(self.work_pool.state(), WorkPoolState::Finished) {
            return Ok(None);
        }

        self.work_pool.get_result(|index| {
            let member = &self.members[index as usize];
            self.inner.seek(SeekFrom::Start(member.start_pos))?;
            let output_limit = if self.mem_limit_kb == u32::MAX {
                None
            } else {
                let mut header = [0; HEADER_SIZE];
                self.inner.read_exact(&mut header)?;
                let header = LzipHeader::parse(&header)?;
                let decoder_kb = get_memory_usage(header.dict_size, 3, 0)?
                    .checked_add(speculation_memory_usage(3, 0))
                    .ok_or_else(|| error_out_of_memory("LZIP member exceeds memory limit"))?;
                let available = u64::from(self.mem_limit_kb)
                    .checked_sub(u64::from(decoder_kb))
                    .and_then(|kb| kb.checked_mul(1024))
                    .and_then(|bytes| bytes.checked_sub(member.compressed_size))
                    .ok_or_else(|| error_out_of_memory("LZIP member exceeds memory limit"))?;
                self.inner.seek(SeekFrom::Start(member.start_pos))?;
                Some(usize::try_from(available).unwrap_or(usize::MAX))
            };
            let size = usize::try_from(member.compressed_size)
                .map_err(|_| error_out_of_memory("LZIP member allocation too large"))?;
            let mut member_data = Vec::new();
            member_data
                .try_reserve_exact(size)
                .map_err(|_| error_out_of_memory("LZIP member allocation too large"))?;
            member_data.resize(size, 0);
            self.inner.read_exact(&mut member_data)?;
            Ok(WorkUnit {
                member_data,
                output_limit,
            })
        })
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
        let work_unit = match worker_handle.steal() {
            Some(work) => {
                active_workers.fetch_add(1, Ordering::Release);
                work
            }
            None => {
                // No more work available and queue is closed.
                break;
            }
        };

        let (
            index,
            WorkUnit {
                member_data,
                output_limit,
            },
        ) = work_unit;

        let result = match decode_member(&member_data, output_limit, &shutdown_flag) {
            Ok(Some(data)) => data,
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

fn decode_member(
    member_data: &[u8],
    output_limit: Option<usize>,
    shutdown_flag: &AtomicBool,
) -> io::Result<Option<Vec<u8>>> {
    let mut lzip_reader = LzipReader::new_single_member(member_data);
    let mut decompressed_data = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        if shutdown_flag.load(Ordering::Acquire) {
            return Ok(None);
        }
        let count = lzip_reader.read(&mut chunk);
        if shutdown_flag.load(Ordering::Acquire) {
            return Ok(None);
        }
        let count = count?;
        if count == 0 {
            return Ok(Some(decompressed_data));
        }
        if output_limit.is_some_and(|limit| decompressed_data.len().saturating_add(count) > limit) {
            return Err(error_out_of_memory("LZIP member exceeds memory limit"));
        }
        decompressed_data
            .try_reserve_exact(count)
            .map_err(|_| error_out_of_memory("LZIP member output allocation too large"))?;
        decompressed_data.extend_from_slice(&chunk[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LzipOptions, LzipWriter, Write};

    #[test]
    fn cancelled_member_stops_before_decoding() {
        let mut writer = LzipWriter::new(Vec::new(), LzipOptions::with_preset(0));
        writer.write_all(&vec![b'x'; 256 * 1024]).unwrap();
        let member = writer.finish().unwrap();
        let shutdown = AtomicBool::new(true);
        assert!(decode_member(&member, None, &shutdown).unwrap().is_none());
    }
}

impl<R: Read + Seek> Read for LzipReaderMt<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let bytes_read = self.current_chunk.read(buf)?;
            if bytes_read > 0 {
                return Ok(bytes_read);
            }

            let Some(chunk_data) = self.get_next_uncompressed_chunk()? else {
                return Ok(0);
            };
            self.current_chunk = Cursor::new(chunk_data);
        }
    }
}
