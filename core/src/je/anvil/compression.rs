use std::{io::Write, path::Path};

use flate2::{
    Compression,
    bufread::{GzDecoder, ZlibDecoder},
    write::{GzEncoder, ZlibEncoder},
};

use crate::error::{Result, WorldPortError, io_result};
use crate::je::{MAX_PAYLOAD_BYTES, read_bounded};

use super::RegionCompression;

/// Compresses a region chunk with the selected output codec.
///
/// # Errors
///
/// Returns an error if the compressor fails.
pub fn compress(kind: RegionCompression, payload: &[u8], level: Compression) -> Result<Vec<u8>> {
    let output = Vec::with_capacity(payload.len() / 2);
    let compressed = match kind {
        RegionCompression::Gzip => {
            let mut encoder = GzEncoder::new(output, level);
            encoder.write_all(payload).and_then(|()| encoder.finish())
        }
        RegionCompression::Zlib => {
            let mut encoder = ZlibEncoder::new(output, level);
            encoder.write_all(payload).and_then(|()| encoder.finish())
        }
        RegionCompression::None => return Ok(payload.to_vec()),
    };
    compressed.map_err(|error| WorldPortError::invalid(format!("compress chunk: {error}")))
}

pub(super) fn decompress(path: &Path, compression: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
    let capacity = payload.len().saturating_mul(3).max(1024);
    let stream = payload.as_slice();
    let decoded = match compression {
        1 => read_bounded(GzDecoder::new(stream), capacity),
        2 => read_bounded(ZlibDecoder::new(stream), capacity),
        3 if stream.len() as u64 <= MAX_PAYLOAD_BYTES => return Ok(payload),
        3 => return Err(WorldPortError::invalid("decoded payload exceeds 64 MiB")),
        4 => read_bounded(lz4_java_wrc::Lz4BlockInput::new(stream), capacity),
        other => {
            return Err(WorldPortError::unsupported(format!(
                "compression ID {other} in {}",
                path.display()
            )));
        }
    };
    io_result(path, decoded)
}
