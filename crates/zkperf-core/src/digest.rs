//! Shared SHA-256 helpers for provenance and artifact integrity.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{ByteSize, Sha256Digest};

const READ_BUFFER_BYTES: usize = 16 * 1024;

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 15)]));
    }
    encoded
}

pub(crate) fn hash_bytes(contents: &[u8]) -> [u8; 32] {
    let mut output = [0_u8; 32];
    output.copy_from_slice(&Sha256::digest(contents));
    output
}

/// Streams a file through SHA-256 without holding its contents in memory.
pub(crate) fn hash_file(path: &Path) -> io::Result<(Sha256Digest, ByteSize)> {
    hash_reader(&mut File::open(path)?)
}

/// Hashes what an already-open handle reads, so the digest describes the bytes
/// that were read rather than whatever the path resolves to afterwards.
pub(crate) fn hash_reader(reader: &mut impl Read) -> io::Result<(Sha256Digest, ByteSize)> {
    copy_and_hash(reader, &mut io::sink())
}

/// Copies and hashes the same bytes, even when the producer changes its file.
pub(crate) fn copy_and_hash(
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<(Sha256Digest, ByteSize)> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    let mut byte_length = 0_u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        byte_length = byte_length
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("file length exceeds the representable range"))?;
    }

    let mut output = [0_u8; 32];
    output.copy_from_slice(&hasher.finalize());
    Ok((Sha256Digest::from_bytes(output), ByteSize::new(byte_length)))
}

#[cfg(test)]
mod tests {
    use super::{encode_hex, hash_bytes};
    use crate::Sha256Digest;

    #[test]
    fn empty_input_has_the_published_sha256_digest() {
        assert_eq!(
            Sha256Digest::from_bytes(hash_bytes(b"")).value(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hex_encoding_pads_every_byte_to_two_lowercase_digits() {
        assert_eq!(encode_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }
}
