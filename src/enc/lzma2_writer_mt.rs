use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use super::{
    encoder::LZMAEncoder,
    range_enc::{RangeEncoder, RangeEncoderBuffer},
    LZMA2Options,
};

const COMPRESSED_SIZE_MAX: u32 = 64 << 10;

/// Represents a chunk of data processed by a worker thread.
/// This does not derive Debug because RangeEncoderBuffer does not.
enum CompressedChunk {
    /// A standard LZMA-compressed chunk. The buffer contains the compressed bytes.
    Lzma {
        uncompressed_size: u32,
        compressed_size: u32,
        buffer: RangeEncoderBuffer,
    },
    /// A chunk that is stored uncompressed because compression was not effective.
    Uncompressed(Vec<u8>),
}

/// The result sent from a worker thread to the writer thread.
type WorkerResult = io::Result<CompressedChunk>;

/// Compresses a single chunk of data.
/// This function is executed by each worker thread.
fn compress_chunk(options: &LZMA2Options, input: Vec<u8>) -> io::Result<CompressedChunk> {
    let uncompressed_size = input.len() as u32;
    if uncompressed_size == 0 {
        // Return early for empty chunks to avoid unnecessary work.
        return Ok(CompressedChunk::Uncompressed(input));
    }

    // Each worker creates its own encoder with an in-memory buffer.
    let mut rc = RangeEncoder::new_buffer(COMPRESSED_SIZE_MAX as usize);
    let (mut lzma, mut mode) = LZMAEncoder::new(
        options.mode,
        options.lc,
        options.lp,
        options.pb,
        options.mf,
        options.depth_limit,
        options.dict_size,
        options.nice_len as usize,
    );

    let mut in_pos = 0;
    while in_pos < input.len() {
        // Fill the window from the current position in the input chunk.
        let used = lzma.lz.fill_window(&input[in_pos..]);
        in_pos += used;

        // Set the finishing flag only when all input has been fed to the encoder.
        if in_pos == input.len() {
            lzma.lz.set_finishing();
        }

        // Encode the data that was just added to the window.
        if lzma.encode_for_lzma2(&mut rc, &mut mode)? {
            // This error indicates the output buffer is full, which should not happen
            // with our parallel chunk design and large COMPRESSED_SIZE_MAX.
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "LZMA2 chunk overflow in worker",
            ));
        }
    }

    // `finish_buffer` finalizes the stream and returns the number of bytes written to the buffer.
    if let Some(compressed_size) = rc.finish_buffer()? {
        // If compression is ineffective, store as uncompressed.
        if (compressed_size as u32) + 2 >= uncompressed_size {
            Ok(CompressedChunk::Uncompressed(input))
        } else {
            // Compression was effective. Extract the buffer containing the data.
            let buffer = rc.into_inner();
            Ok(CompressedChunk::Lzma {
                uncompressed_size,
                compressed_size: compressed_size as u32,
                buffer,
            })
        }
    } else {
        // No bytes were written, store as uncompressed.
        Ok(CompressedChunk::Uncompressed(input))
    }
}

/// A multi-threaded LZMA2 encoder.
///
/// This writer uses a pool of worker threads to compress data in parallel and
/// a dedicated writer thread to write the compressed data to the underlying stream.
pub struct MTLZMA2Writer<W: Write + Send + 'static> {
    writer_thread: Option<thread::JoinHandle<io::Result<W>>>,
    input_tx: mpsc::Sender<Vec<u8>>,
    has_error: Arc<AtomicBool>,
    buffer: Vec<u8>,
    chunk_size: usize,
}

impl<W: Write + Send + 'static> MTLZMA2Writer<W> {
    /// Creates a new multi-threaded LZMA2 writer.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying writer to write compressed data to.
    /// * `options` - LZMA2 compression options.
    /// * `threads` - The number of worker threads to use for compression.
    pub fn new(mut inner: W, options: &LZMA2Options, threads: u32) -> Self {
        let chunk_size = (options.dict_size as usize * 2).min(1 << 22); // Max 4MB chunks

        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        let (output_tx, output_rx) = mpsc::channel::<WorkerResult>();
        let input_rx = Arc::new(Mutex::new(input_rx));
        let has_error = Arc::new(AtomicBool::new(false));
        let options_arc = Arc::new(options.clone());

        // Spawn worker threads
        for _ in 0..threads.max(1) {
            let input_rx = Arc::clone(&input_rx);
            let output_tx = output_tx.clone();
            let options_arc = Arc::clone(&options_arc);
            let has_error = Arc::clone(&has_error);

            thread::spawn(move || {
                loop {
                    if has_error.load(Ordering::SeqCst) {
                        break;
                    }

                    let input_chunk = match input_rx.lock().unwrap().recv() {
                        Ok(chunk) => chunk,
                        Err(_) => break, // Channel closed.
                    };

                    let result = compress_chunk(&options_arc, input_chunk);

                    if result.is_err() {
                        has_error.store(true, Ordering::SeqCst);
                    }
                    if output_tx.send(result).is_err() {
                        has_error.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            });
        }
        drop(output_tx);

        // Spawn the writer thread
        let writer_has_error = Arc::clone(&has_error);
        let props = options.get_props();
        let writer_thread = thread::spawn(move || -> io::Result<W> {
            // State is managed per-chunk within the loop.
            let mut props_needed = true;
            let mut dict_reset_needed = true;
            let mut state_reset_needed = true;

            for result in output_rx {
                if writer_has_error.load(Ordering::SeqCst) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Encoding aborted due to worker thread error.",
                    ));
                }

                match result {
                    Ok(chunk) => {
                        // Because each chunk is compressed independently, the dictionary
                        // MUST be reset for every chunk. The only exception is a sequence of
                        // uncompressed chunks.
                        let write_res = match chunk {
                            CompressedChunk::Lzma {
                                uncompressed_size,
                                compressed_size,
                                buffer,
                            } => {
                                // For an LZMA chunk, we always reset the dictionary in this parallel design.
                                let res = write_lzma_chunk(
                                    &mut inner,
                                    uncompressed_size,
                                    compressed_size,
                                    &buffer,
                                    props,
                                    &mut props_needed,
                                    &mut true,
                                    &mut state_reset_needed,
                                );
                                // After an LZMA chunk, the next uncompressed chunk would need a dict reset.
                                dict_reset_needed = true;
                                res
                            }
                            CompressedChunk::Uncompressed(data) => {
                                if data.is_empty() {
                                    continue;
                                }
                                let res = write_uncompressed_chunk(
                                    &mut inner,
                                    &data,
                                    &mut dict_reset_needed,
                                    &mut state_reset_needed,
                                );
                                // After an uncompressed chunk, a subsequent uncompressed chunk
                                // does NOT need a dictionary reset.
                                res
                            }
                        };
                        if let Err(e) = write_res {
                            writer_has_error.store(true, Ordering::SeqCst);
                            return Err(e);
                        }
                    }
                    Err(e) => {
                        writer_has_error.store(true, Ordering::SeqCst);
                        return Err(e);
                    }
                }
            }

            // End of stream marker
            inner.write_all(&[0x00])?;

            Ok(inner)
        });

        MTLZMA2Writer {
            writer_thread: Some(writer_thread),
            input_tx,
            has_error,
            buffer: Vec::with_capacity(chunk_size),
            chunk_size,
        }
    }

    fn check_error(&self) -> io::Result<()> {
        if self.has_error.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "An error occurred in a background thread.",
            ))
        } else {
            Ok(())
        }
    }

    fn send_buffer(&mut self) -> io::Result<()> {
        self.check_error()?;
        if !self.buffer.is_empty() {
            let chunk = std::mem::take(&mut self.buffer);
            if self.input_tx.send(chunk).is_err() {
                self.has_error.store(true, Ordering::SeqCst);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "LZMA2 worker threads have terminated.",
                ));
            }
            self.buffer.reserve(self.chunk_size);
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.send_buffer()?;
        drop(self.input_tx);

        match self.writer_thread.take().unwrap().join() {
            Ok(writer_result) => writer_result,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "LZMA2 writer thread panicked",
            )),
        }
    }
}

impl<W: Write + Send + 'static> Write for MTLZMA2Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check_error()?;
        self.buffer.extend_from_slice(buf);

        while self.buffer.len() >= self.chunk_size {
            let chunk_to_send = self.buffer.drain(..self.chunk_size).collect::<Vec<u8>>();

            if self.input_tx.send(chunk_to_send).is_err() {
                self.has_error.store(true, Ordering::SeqCst);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "LZMA2 worker threads have terminated.",
                ));
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_buffer()
    }
}

/// Helper function to write a compressed LZMA chunk to the stream.
fn write_lzma_chunk<W: Write>(
    inner: &mut W,
    uncompressed_size: u32,
    compressed_size: u32,
    buffer: &RangeEncoderBuffer,
    props: u8,
    props_needed: &mut bool,
    dict_reset_needed: &mut bool,
    state_reset_needed: &mut bool,
) -> io::Result<()> {
    let control = if *props_needed {
        if *dict_reset_needed {
            0x80 + (3 << 5)
        } else {
            0x80 + (2 << 5)
        }
    } else if *state_reset_needed {
        0x80 + (1 << 5)
    } else {
        0x80
    };
    let control = control | ((uncompressed_size - 1) >> 16) as u8;

    let mut chunk_header = [0u8; 6];
    chunk_header[0] = control;
    chunk_header[1] = ((uncompressed_size - 1) >> 8) as u8;
    chunk_header[2] = (uncompressed_size - 1) as u8;
    chunk_header[3] = ((compressed_size - 1) >> 8) as u8;
    chunk_header[4] = (compressed_size - 1) as u8;

    if *props_needed {
        chunk_header[5] = props;
        inner.write_all(&chunk_header)?;
    } else {
        inner.write_all(&chunk_header[..5])?;
    }

    // Use the buffer's dedicated write_to method.
    buffer.write_to(inner)?;

    *props_needed = false;
    *state_reset_needed = false;
    *dict_reset_needed = false;
    Ok(())
}

/// Helper function to write an uncompressed chunk to the stream.
fn write_uncompressed_chunk<W: Write>(
    inner: &mut W,
    data: &[u8],
    dict_reset_needed: &mut bool,
    state_reset_needed: &mut bool,
) -> io::Result<()> {
    let mut uncompressed_size = data.len();
    let mut pos = 0;

    while uncompressed_size > 0 {
        let chunk_size = uncompressed_size.min(COMPRESSED_SIZE_MAX as usize);
        let mut chunk_header = [0u8; 3];
        chunk_header[0] = if *dict_reset_needed { 0x01 } else { 0x02 };
        chunk_header[1] = ((chunk_size as u32 - 1) >> 8) as u8;
        chunk_header[2] = (chunk_size as u32 - 1) as u8;
        inner.write_all(&chunk_header)?;
        inner.write_all(&data[pos..(pos + chunk_size)])?;

        pos += chunk_size;
        uncompressed_size -= chunk_size;
        *dict_reset_needed = false;
    }

    *state_reset_needed = true;
    Ok(())
}
