//! Randomness and hashing for credentials (kernel spec §3, §4).

use sha2::{Digest, Sha256};

/// `N` bytes from the operating system's CSPRNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("the OS random number generator is available");
    out
}

/// Lowercase hex SHA-256: how pairing codes (and later session ids) are
/// stored at rest.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Seconds since the Unix epoch, the kernel tables' time unit.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_differ_between_calls() {
        assert_ne!(random_bytes::<32>(), random_bytes::<32>());
    }

    #[test]
    fn sha256_hex_matches_the_known_digest_of_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
