//! Wire format: length-prefixed sealed blobs. Each blob is independently
//! encrypted and authenticated, so a stream can be read chunk by chunk.

use crate::crypto::{seal, unseal, SecretKey};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const MAX_BLOB: usize = 8 * 1024 * 1024;
pub const CHUNK: usize = 256 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileMeta {
    pub id: String,
    pub name: String,
    pub size: u64,
}

/// A device shouting "I am here" on the LAN. Sealed, so only devices with
/// the same room key can even read the name.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Announce {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub port: u16,
    pub ts: u64,
    /// App version and a fingerprint of the running executable, so devices
    /// can spot that one of them is newer and pull the update from it.
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub exe_sha: String,
    /// Which build an update from this device would be usable by.
    #[serde(default)]
    pub platform_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t")]
pub enum Frame {
    /// A message: text, attachment offers, or both.
    Post {
        msg_id: String,
        from_id: String,
        from_name: String,
        from_port: u16,
        ts: u64,
        body: String,
        #[serde(default)]
        files: Vec<FileMeta>,
    },
    /// "Send me the file you offered." Attachments are pulled, never pushed,
    /// so nothing is ever written to a disk without someone asking for it.
    Fetch { file_id: String },
    /// "Send me your copy of the app." Replied to exactly like a file.
    FetchUpdate,
    /// Reply to Fetch or FetchUpdate, followed by the content's chunks.
    FetchOk { name: String, size: u64 },
    FetchErr { reason: String },
}

pub fn write_blob<W: Write>(w: &mut W, key: &SecretKey, plain: &[u8]) -> io::Result<()> {
    let sealed = seal(key, plain);
    w.write_all(&(sealed.len() as u32).to_be_bytes())?;
    w.write_all(&sealed)
}

/// Ok(None) means the other side closed cleanly.
pub fn read_blob<R: Read>(r: &mut R, key: &SecretKey) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_BLOB {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad blob length"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    unseal(key, &buf)
        .map(Some)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "wrong room key"))
}

pub fn write_frame<W: Write>(w: &mut W, key: &SecretKey, f: &Frame) -> io::Result<()> {
    let json = serde_json::to_vec(f)?;
    write_blob(w, key, &json)
}

pub fn read_frame<R: Read>(r: &mut R, key: &SecretKey) -> io::Result<Option<Frame>> {
    match read_blob(r, key)? {
        None => Ok(None),
        Some(bytes) => {
            let f = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(f))
        }
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
