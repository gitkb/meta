//! Paths on the wire. JSON strings are UTF-8 and a path need not be (a
//! Unix folder name is any bytes), so a path is sent as its string when it
//! is UTF-8, and otherwise as `{"lossy": "<what it shows as>", "bytes":
//! "<base64 of its bytes>"}`. A path read back is the one sent, byte for
//! byte, on Unix; elsewhere paths are UTF-16 and a non-UTF-8 one never
//! reaches the wire.
//!
//! Used with `#[serde(with = "crate::wire_path")]` on a `PathBuf` field
//! (and `wire_path::option` on an `Option<PathBuf>`).

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Wire {
    Text(String),
    Bytes { lossy: String, bytes: String },
}

/// The base64 of `path`'s bytes, when it is not UTF-8: what `path_bytes`
/// holds beside a path shown as text.
pub fn bytes_of(path: &Path) -> Option<String> {
    match path.to_str() {
        Some(_) => None,
        None => Some(base64::engine::general_purpose::STANDARD.encode(raw(path))),
    }
}

/// The path whose bytes `encoded` holds (base64, as [`bytes_of`] writes).
pub fn from_bytes(encoded: &str) -> Result<PathBuf, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("not base64: {e}"))?;
    path_of(bytes)
}

#[cfg(unix)]
fn raw(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn raw(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(unix)]
fn path_of(bytes: Vec<u8>) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_of(bytes: Vec<u8>) -> Result<PathBuf, String> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| "a path that is not UTF-8 has no form here".to_string())
}

fn wire(path: &Path) -> Wire {
    match bytes_of(path) {
        None => Wire::Text(path.to_string_lossy().into_owned()),
        Some(bytes) => Wire::Bytes {
            lossy: path.to_string_lossy().into_owned(),
            bytes,
        },
    }
}

fn unwire(wire: Wire) -> Result<PathBuf, String> {
    match wire {
        Wire::Text(text) => Ok(PathBuf::from(text)),
        Wire::Bytes { bytes, .. } => from_bytes(&bytes),
    }
}

pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    wire(path).serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    unwire(Wire::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

/// The same for an `Option<PathBuf>` (`null` for none).
pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_deref().map(wire).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<Wire>::deserialize(deserializer)?
            .map(unwire)
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Holder {
        #[serde(with = "crate::wire_path")]
        path: PathBuf,
    }

    fn round_trip(path: &Path) -> (serde_json::Value, PathBuf) {
        let sent = serde_json::to_value(Holder {
            path: path.to_path_buf(),
        })
        .unwrap();
        let got: Holder = serde_json::from_value(sent.clone()).unwrap();
        (sent, got.path)
    }

    #[test]
    fn a_utf8_path_is_its_string() {
        let (sent, got) = round_trip(Path::new("/w/a b/ünï"));
        assert_eq!(sent, serde_json::json!({"path": "/w/a b/ünï"}));
        assert_eq!(got, Path::new("/w/a b/ünï"));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_arrives_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/w/caf\xe9"));
        let (sent, got) = round_trip(path);
        assert_eq!(sent["path"]["lossy"], "/w/caf\u{fffd}");
        assert_eq!(got, path);
        assert_eq!(from_bytes(&bytes_of(path).unwrap()).unwrap(), path);
    }
}
