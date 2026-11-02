//! Sealed sync files (design §3.3).
//!
//! ```text
//! file = "EPHS" | version: u8 | replica: u64 (big-endian) | nonce: 24 bytes
//!        | XChaCha20-Poly1305(key, nonce, aad = "EPHS" | version | replica,
//!                             plaintext = the canonical state JSON)
//! ```
//!
//! The header is authenticated, so a file renamed onto another replica's
//! name, or changed in any byte, fails to open. The caller supplies the
//! nonce (random, from the platform), keeping this crate free of randomness
//! and testable with fixed nonces.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::hlc::ReplicaId;
use crate::state::SyncState;

pub const MAGIC: &[u8; 4] = b"EPHS";
pub const ENVELOPE_VERSION: u8 = 1;
pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 4 + 1 + 8;
const TAG_LEN: usize = 16;

/// The sync key: 32 random bytes, generated once and shared by pairing.
pub type Key = [u8; KEY_LEN];

fn header(replica: ReplicaId) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(MAGIC);
    h[4] = ENVELOPE_VERSION;
    h[5..].copy_from_slice(&replica.to_be_bytes());
    h
}

/// `state`, sealed as `replica`'s file.
pub fn seal(state: &SyncState, replica: ReplicaId, key: &Key, nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let aad = header(replica);
    let body = XChaCha20Poly1305::new(key.into())
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: state.to_json().as_bytes(),
                aad: &aad,
            },
        )
        .expect("XChaCha20-Poly1305 encrypts any message under 256 GiB");
    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + body.len());
    out.extend_from_slice(&aad);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&body);
    out
}

/// The replica id and validated state in a sealed file.
pub fn open(bytes: &[u8], key: &Key) -> Result<(ReplicaId, SyncState), String> {
    if bytes.len() < HEADER_LEN + NONCE_LEN + TAG_LEN || &bytes[..4] != MAGIC {
        return Err("not an Ephemeris sync file".into());
    }
    if bytes[4] != ENVELOPE_VERSION {
        return Err(format!(
            "sync file format {} (this build reads {ENVELOPE_VERSION}); update the app",
            bytes[4]
        ));
    }
    let (aad, rest) = bytes.split_at(HEADER_LEN);
    let (nonce, body) = rest.split_at(NONCE_LEN);
    let replica = ReplicaId::from_be_bytes(aad[5..].try_into().expect("8 bytes"));
    let plain = XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad })
        .map_err(|_| "this sync file is from another key, or damaged".to_string())?;
    let text = std::str::from_utf8(&plain).map_err(|_| "sync state is not UTF-8".to_string())?;
    Ok((replica, SyncState::from_json(text)?))
}

/// Crockford base32 (no padding), for showing and typing the key.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The key as 52 base32 characters in groups of four, for pairing (§3.3).
pub fn key_to_text(key: &Key) -> String {
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in key {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("ASCII"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse a typed key: case, spaces and dashes are ignored, and the
/// look-alikes O, I and L read as 0, 1 and 1 (Crockford).
pub fn key_from_text(text: &str) -> Result<Key, String> {
    let bad = || "that is not a sync key (52 characters, 0-9 and A-Z)".to_string();
    let mut bytes = Vec::with_capacity(KEY_LEN);
    let (mut acc, mut bits, mut n) = (0u32, 0u32, 0usize);
    for ch in text.chars().filter(|c| !c.is_whitespace() && *c != '-') {
        let c = match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        let v = ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or_else(bad)? as u32;
        acc = ((acc << 5) | v) & 0xFFFF;
        bits += 5;
        n += 1;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
        }
    }
    // 52 characters carry 260 bits: 256 of key and 4 zero bits.
    if n != 52 || acc & ((1 << bits) - 1) != 0 {
        return Err(bad());
    }
    bytes.try_into().map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lww::Lww;
    use crate::state::Setting;

    fn state() -> SyncState {
        let mut s = SyncState::new();
        let at = s.stamp(0xabc, 1_000);
        s.settings.insert(Setting::HorizonDays, Lww::new(at, 180));
        s
    }

    const KEY: Key = [7; 32];
    const NONCE: [u8; 24] = [9; 24];

    #[test]
    fn seal_then_open_round_trips() {
        let s = state();
        let file = seal(&s, 0xabc, &KEY, &NONCE);
        assert_eq!(&file[..4], b"EPHS");
        assert_eq!(file.len(), 13 + 24 + s.to_json().len() + 16);
        let (replica, back) = open(&file, &KEY).unwrap();
        assert_eq!((replica, back), (0xabc, s));
    }

    #[test]
    fn the_same_inputs_seal_to_the_same_bytes_and_new_nonces_differ() {
        let s = state();
        assert_eq!(seal(&s, 1, &KEY, &NONCE), seal(&s, 1, &KEY, &NONCE));
        assert_ne!(seal(&s, 1, &KEY, &NONCE), seal(&s, 1, &KEY, &[8; 24]));
    }

    #[test]
    fn wrong_key_tampering_renaming_and_truncation_all_fail() {
        let file = seal(&state(), 0xabc, &KEY, &NONCE);
        assert!(open(&file, &[8; 32]).unwrap_err().contains("another key"));
        for i in [0, 4, 6, 20, 40, file.len() - 1] {
            let mut bad = file.clone();
            bad[i] ^= 1;
            assert!(open(&bad, &KEY).is_err(), "flipped byte {i}");
        }
        // Claiming to be another replica's file breaks the authenticated header.
        let mut renamed = file.clone();
        renamed[5..13].copy_from_slice(&0xdefu64.to_be_bytes());
        assert!(open(&renamed, &KEY).unwrap_err().contains("another key"));
        for n in [0, 3, 13, 36, 52, file.len() - 1] {
            assert!(open(&file[..n], &KEY).is_err(), "truncated to {n}");
        }
    }

    #[test]
    fn unknown_versions_are_refused_by_name() {
        let mut file = seal(&state(), 1, &KEY, &NONCE);
        file[4] = 2;
        assert!(open(&file, &KEY).unwrap_err().contains("update the app"));
    }

    #[test]
    fn keys_round_trip_through_text() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        let text = key_to_text(&key);
        assert_eq!(text.replace('-', "").len(), 52);
        assert_eq!(key_from_text(&text), Ok(key));
        assert_eq!(
            key_from_text(&text.to_lowercase().replace('-', " ")),
            Ok(key)
        );
        assert_eq!(key_to_text(&[0; 32]), ["0000"; 13].join("-"));
        assert_eq!(key_to_text(&[0xff; 32]).chars().last(), Some('G')); // 4 pad bits
        let typo = text.replacen(|c: char| c.is_ascii_alphanumeric(), "U", 1);
        assert!(key_from_text(&typo).is_err(), "U is not in the alphabet");
        assert!(key_from_text(&text[..text.len() - 1]).is_err());
        assert!(key_from_text(&format!("{text}0")).is_err());
    }

    #[test]
    fn look_alike_letters_read_as_digits() {
        let text = key_to_text(&[0; 32])
            .replace('0', "o")
            .replacen('o', "O", 1);
        assert_eq!(key_from_text(&text), Ok([0; 32]));
    }
}
