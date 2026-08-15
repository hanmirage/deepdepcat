//! LSP wire protocol — JSON-RPC framing over stdio.
//!
//! Implements the LSP transport layer: `Content-Length` headers + JSON-RPC
//! messages (requests, notifications, responses) plus path↔URI conversion.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Encode a JSON-RPC message body into an LSP transport frame.
pub fn encode_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(body.len() + 64);
    frame.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Read one complete frame (header + body) from a buffered reader.
///
/// Returns the raw JSON body as bytes. `Ok(None)` means EOF before any
/// frame started. A frame that starts but is truncated returns an error.
pub async fn read_frame<R: tokio::io::AsyncBufReadExt + tokio::io::AsyncReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut header_line = String::new();
    let mut content_length: Option<usize> = None;

    loop {
        header_line.clear();
        let n = reader.read_line(&mut header_line).await?;
        if n == 0 {
            return if content_length.is_some() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "truncated LSP frame: EOF in headers",
                ))
            } else {
                Ok(None)
            };
        }
        let line = header_line.trim_end_matches("\r\n").trim_end_matches('\n');
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "bad Content-Length")
            })?);
        }
        // Other headers (Content-Type) are ignored.
    }

    let len = content_length.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length")
    })?;

    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Build a JSON-RPC request message.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// Build a JSON-RPC notification message.
pub fn notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// Extract the request id from a response/request message.
pub fn message_id(msg: &Value) -> Option<u64> {
    msg.get("id").and_then(|id| id.as_u64()).or_else(|| {
        msg.get("id")
            .and_then(|id| id.as_str())
            .and_then(|s| s.parse().ok())
    })
}

/// Extract a result from a response message (`None` when the message is a
/// request/notification or carried an error).
pub fn response_result(msg: &Value) -> Option<&Value> {
    msg.get("id")?;
    if msg.get("error").is_some() {
        return None;
    }
    msg.get("result")
}

/// Extract the error text from a response message, if any.
pub fn response_error(msg: &Value) -> Option<String> {
    msg.get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(String::from)
}

// ── Path ↔ URI ──────────────────────────────────────────────────────────────

/// Convert a filesystem path to an LSP document URI (`file://` scheme).
///
/// Windows paths become `/C:/…` with a **lowercase drive letter**
/// (`/c:/…`) — rust-analyzer and other servers normalize drive letters to
/// lowercase, and URI comparison is case-sensitive. Spaces and other
/// reserved characters are percent-encoded.
pub fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let normalized = if cfg!(windows) && !raw.starts_with('/') {
        let (drive, rest) = raw.split_at(2);
        if drive.ends_with(':') {
            format!("/{}{rest}", drive.to_ascii_lowercase())
        } else {
            format!("/{raw}")
        }
    } else {
        raw
    };
    let encoded: Vec<String> = normalized
        .split('/')
        .map(|seg| {
            let is_drive = seg.len() == 2 && seg.ends_with(':');
            if seg.is_empty() || is_drive {
                seg.to_string()
            } else {
                urlencoding::encode(seg).into_owned()
            }
        })
        .collect();
    format!("file://{}", encoded.join("/"))
}

/// Convert an LSP document URI back to a filesystem path.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = rest
        .split('/')
        .map(|seg| {
            urlencoding::decode(seg)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| seg.to_string())
        })
        .collect::<Vec<String>>()
        .join("/");
    if cfg!(windows) {
        // /C:/x → C:\x
        let trimmed = decoded.trim_start_matches('/');
        Some(PathBuf::from(trimmed.replace('/', "\\")))
    } else {
        Some(PathBuf::from(format!("/{decoded}")))
    }
}

/// Position inside a document (0-based line + character).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A range in a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// Normalized severity: 1 = Error, 2 = Warning, 3 = Info, 4 = Hint.
pub fn severity_label(severity: Option<u64>) -> &'static str {
    match severity {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_encoding_roundtrip() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let frame = encode_frame(body);
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with(&format!("Content-Length: {}\r\n\r\n", body.len())));
        assert!(text.ends_with(std::str::from_utf8(body).unwrap()));
    }

    #[tokio::test]
    async fn frame_reading_roundtrip() {
        let body = br#"{"jsonrpc":"2.0","method":"exit"}"#;
        let frame = encode_frame(body);
        let mut reader = tokio::io::BufReader::new(&frame[..]);
        let read = read_frame(&mut reader).await.unwrap();
        assert_eq!(read.as_deref(), Some(body.as_slice()));
    }

    #[tokio::test]
    async fn frame_reading_eof_returns_none() {
        let mut reader = tokio::io::BufReader::new(&b""[..]);
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn frame_reading_multiple_frames() {
        let body = br#"{"a":1}"#;
        let mut data = encode_frame(body);
        data.extend_from_slice(&encode_frame(br#"{"b":2}"#));
        let mut reader = tokio::io::BufReader::new(&data[..]);
        assert_eq!(
            read_frame(&mut reader).await.unwrap().as_deref(),
            Some(body.as_slice())
        );
        assert_eq!(
            read_frame(&mut reader).await.unwrap().as_deref(),
            Some(br#"{"b":2}"#.as_slice())
        );
    }

    #[test]
    fn path_to_uri_windows_style() {
        let uri = path_to_uri(Path::new("C:\\my project\\src\\main.rs"));
        assert!(uri.starts_with("file:///c:/"), "got: {uri}");
        assert!(uri.contains("my%20project"));
        assert!(uri.ends_with("/src/main.rs"));
    }

    #[test]
    fn uri_to_path_roundtrip() {
        let path = Path::new("C:\\my project\\src\\main.rs");
        let uri = path_to_uri(path);
        let back = uri_to_path(&uri).unwrap();
        assert_eq!(back, path);
    }

    #[test]
    fn uri_to_path_percent_decoding() {
        let back = uri_to_path("file:///home/user/a%20b.rs").unwrap();
        assert!(
            back.to_string_lossy().ends_with("a b.rs"),
            "got: {}",
            back.display()
        );
    }

    #[test]
    fn severity_mapping() {
        assert_eq!(severity_label(Some(1)), "error");
        assert_eq!(severity_label(Some(2)), "warning");
        assert_eq!(severity_label(Some(3)), "info");
        assert_eq!(severity_label(Some(4)), "hint");
        assert_eq!(severity_label(None), "error");
    }

    #[test]
    fn message_id_extraction() {
        assert_eq!(message_id(&json!({"id": 42})), Some(42));
        assert_eq!(message_id(&json!({"id": "9"})), Some(9));
        assert_eq!(message_id(&json!({"method": "x"})), None);
    }

    #[test]
    fn response_result_and_error() {
        assert_eq!(
            response_result(&json!({"id": 1, "result": {"ok": true}})),
            Some(&json!({"ok": true}))
        );
        assert!(response_result(&json!({"id": 1, "error": {"message": "boom"}})).is_none());
        assert!(response_result(&json!({"method": "x"})).is_none());
        assert_eq!(
            response_error(&json!({"id": 1, "error": {"message": "boom"}})),
            Some("boom".to_string())
        );
    }
}
