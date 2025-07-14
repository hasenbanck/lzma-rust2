#[cfg(feature = "optimization")]
mod aligned_memory;
mod bt4;
mod hash234;
mod hc4;
mod lz_decoder;
mod lz_encoder;

#[cfg(feature = "optimization")]
pub(crate) use aligned_memory::*;
pub(crate) use lz_decoder::*;
pub use lz_encoder::*;
