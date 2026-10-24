//! Everything on the wire is sealed with a key derived from the room key,
//! so an app only ever talks to other apps that share the same secret.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};

pub type SecretKey = [u8; 32];

pub fn derive_key(room_key: &str) -> SecretKey {
    let mut h = Sha256::new();
    h.update(b"netpaste-room-v1\0");
    h.update(room_key.trim().as_bytes());
    h.finalize().into()
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    getrandom::getrandom(&mut v).expect("system rng unavailable");
    v
}

pub fn random_hex(bytes: usize) -> String {
    random_bytes(bytes).iter().map(|b| format!("{:02x}", b)).collect()
}

/// A human-typable room key: five short words plus a number.
pub fn suggest_room_key() -> String {
    const WORDS: [&str; 32] = [
        "amber", "anchor", "basil", "cedar", "cobalt", "copper", "coral", "delta",
        "ember", "fable", "flint", "garnet", "harbor", "indigo", "ivory", "juniper",
        "kestrel", "lantern", "marble", "nickel", "onyx", "opal", "pepper", "quartz",
        "raven", "saffron", "slate", "thistle", "umber", "velvet", "willow", "zephyr",
    ];
    let r = random_bytes(4);
    format!(
        "{}-{}-{}-{}",
        WORDS[(r[0] & 31) as usize],
        WORDS[(r[1] & 31) as usize],
        WORDS[(r[2] & 31) as usize],
        100 + (r[3] as u16) * 3 % 900
    )
}

pub fn seal(key: &SecretKey, plain: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = random_bytes(24);
    let mut out = nonce.clone();
    out.extend(
        cipher
            .encrypt(XNonce::from_slice(&nonce), plain)
            .expect("encryption failed"),
    );
    out
}

/// Returns None when the data was not sealed with this key - which is also
/// how we silently ignore traffic from a different room.
pub fn unseal(key: &SecretKey, data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 24 {
        return None;
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher.decrypt(XNonce::from_slice(&data[..24]), &data[24..]).ok()
}
