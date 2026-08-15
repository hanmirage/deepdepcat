//! Output decoding for child processes on Windows.
//!
//! Console applications (cmd fallback, native tools) write text in the
//! system OEM code page (GBK on zh-CN Windows), while PowerShell 7 and
//! modern tools write UTF-8. Blindly decoding with `String::from_utf8_lossy`
//! mangles the GBK bytes into U+FFFD replacement characters. `decode_native_output`
//! detects the encoding per buffer: strict UTF-8 wins, then UTF-16 BOMs,
//! then GBK (the dominant Windows legacy code page for CJK systems).

use encoding_rs::GBK;

/// Decode bytes produced by a child process into a lossless String.
///
/// Strategy per buffer:
/// 1. UTF-8 BOM → strip and decode as UTF-8 (lossy tail).
/// 2. UTF-16 LE/BE BOM → decode as UTF-16.
/// 3. Valid UTF-8 → use it directly.
/// 4. Otherwise → decode as GBK (Windows CJK code page), falling back to
///    lossy UTF-8 if even GBK fails (e.g. binary garbage).
pub fn decode_native_output(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16_to_string(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16_to_string(rest, true);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let (decoded, _, _) = GBK.decode(bytes);
    if !decoded.contains('\u{FFFD}') {
        return decoded.into_owned();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Decode UTF-16 (LE or BE, no BOM) bytes into a String.
fn utf16_to_string(bytes: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            let raw = u16::from_le_bytes([pair[0], pair[1]]);
            if big_endian {
                raw.swap_bytes()
            } else {
                raw
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Length of a trailing byte run that is an INCOMPLETE multi-byte char.
///
/// Streaming readers slice output at arbitrary byte offsets; if a UTF-8 or
/// GBK character straddles the slice boundary, decoding the slice alone
/// corrupts it (a UTF-8 lead re-decoded as a different GBK char, or a lone
/// lead → U+FFFD). Returning the incomplete run lets the reader HOLD those
/// bytes back and re-read them with the next chunk — a char is never decoded
/// from a partial sequence.
///
/// - Valid UTF-8 → 0 (ends on a boundary).
/// - Incomplete UTF-8 tail (`error_len == None`) → the tail's byte count.
/// - Otherwise (GBK text): walk as GBK — high bytes pair up (lead + trail);
///   a trailing unpaired lead means its second half is in the next read.
pub fn incomplete_trailing_bytes(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    match std::str::from_utf8(buf) {
        Ok(_) => 0,
        Err(e) if e.error_len().is_none() => buf.len() - e.valid_up_to(),
        Err(_) => {
            let mut expecting_trail = false;
            for &b in buf {
                expecting_trail = b >= 0x80 && !expecting_trail;
            }
            usize::from(expecting_trail)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_passthrough() {
        assert_eq!(decode_native_output("中文 utf8".as_bytes()), "中文 utf8");
        assert_eq!(decode_native_output(b"plain ascii"), "plain ascii");
    }

    #[test]
    fn gbk_is_decoded() {
        // "中文测试" encoded as GBK bytes.
        let gbk = [0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4];
        let decoded = decode_native_output(&gbk);
        assert_eq!(decoded, "中文测试");
    }

    #[test]
    fn mixed_gbk_and_utf8_line() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xD6, 0xD0]); // 中 (GBK)
        bytes.extend_from_slice(b" : ok");
        let decoded = decode_native_output(&bytes);
        assert!(decoded.starts_with('中'));
        assert!(decoded.contains(": ok"));
    }

    #[test]
    fn utf8_bom_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("内容".as_bytes());
        assert_eq!(decode_native_output(&bytes), "内容");
    }

    #[test]
    fn utf16le_bom_decoded() {
        let text = "界面";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_native_output(&bytes), "界面");
    }

    #[test]
    fn binary_garbage_falls_back_to_lossy() {
        let garbage = [0x00, 0x01, 0x02, 0xFF, 0xFE];
        // Must not panic; lossy conversion of bytes that are neither valid
        // UTF-8 nor decodable GBK.
        let _ = decode_native_output(&garbage);
    }

    #[test]
    fn incomplete_trailing_bytes_holds_partial_chars() {
        // Valid UTF-8 ends on a boundary → 0.
        assert_eq!(incomplete_trailing_bytes("中文".as_bytes()), 0);
        // A 3-byte UTF-8 char cut after 2 bytes → hold those 2.
        // "文" = E6 96 87; cut at E6 96.
        assert_eq!(incomplete_trailing_bytes(&[0xE6, 0x96]), 2);
        // Cut after 1 lead byte → hold 1.
        assert_eq!(incomplete_trailing_bytes(&[0xE6]), 1);
        // Empty → 0.
        assert_eq!(incomplete_trailing_bytes(&[]), 0);
        // GBK: complete pair "中" (D6 D0) ends on a boundary → 0.
        assert_eq!(incomplete_trailing_bytes(&[0xD6, 0xD0]), 0);
        // GBK: trailing lone lead (CE) → hold 1.
        assert_eq!(incomplete_trailing_bytes(&[0xD6, 0xD0, 0xCE]), 1);
        // GBK text with an ASCII suffix ends cleanly.
        assert_eq!(incomplete_trailing_bytes(&[0xD6, 0xD0, b':', b'x']), 0);
    }
}
