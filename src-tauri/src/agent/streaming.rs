//! UTF-8 safe streaming chunker — ported from xai-tool-runtime.
//!
//! `stream_chunk()` takes a monotonic byte source (tail buffer + total bytes)
//! and produces `ToolProgress::PartialResult` deltas that are always valid
//! UTF-8 and lossless. Incomplete multi-byte sequences at the chunk boundary
//! are held back for the next call.

use serde::{Deserialize, Serialize};

use crate::toolkit::ToolProgress;

/// Per-frame delta byte cap when `max_delta_bytes` is unset.
const DEFAULT_MAX_DELTA_BYTES: usize = 16 * 1024;

/// Canonical payload for streaming tool output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialResultPayload {
    pub delta: String,
    pub total_bytes: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub gap: bool,
}

/// Byte count of an incomplete UTF-8 sequence at the end of `bytes`, or 0.
fn incomplete_utf8_suffix_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => 0,
        Err(e) if e.error_len().is_none() => bytes.len() - e.valid_up_to(),
        Err(_) => 0,
    }
}

/// Build at most one `ToolProgress::PartialResult` delta from a monotonic byte
/// source, with UTF-8-safe slicing at both the tick boundary and the per-frame
/// cap.
///
/// Deltas are **append-only and lossless**: when a delta would end mid-way
/// through a multi-byte UTF-8 sequence, or exceeds the per-frame cap, the excess
/// bytes are *held back* — `last_total` advances only past the emitted bytes,
/// so the next call re-slices the remainder. Concatenated deltas are always
/// valid UTF-8 and lossless.
pub fn stream_chunk(
    max_delta_bytes: Option<usize>,
    tail: &[u8],
    total: u64,
    last_total: &mut u64,
    truncated: bool,
) -> Option<ToolProgress> {
    if total <= *last_total {
        return None;
    }
    let new = total - *last_total;
    let tail_len = tail.len() as u64;

    let (delta_bytes, gap) = if new <= tail_len {
        (&tail[(tail_len - new) as usize..], false)
    } else {
        (tail, true)
    };

    let cap = max_delta_bytes.unwrap_or(DEFAULT_MAX_DELTA_BYTES);

    // Defer: emit the longest prefix that fits the cap AND ends on a complete
    // UTF-8 sequence; hold the rest back for the next call.
    let mut cut = delta_bytes.len().min(cap);
    while cut > 0 && incomplete_utf8_suffix_len(&delta_bytes[..cut]) > 0 {
        cut -= 1;
    }
    // A cap smaller than one multi-byte char would deadlock; emit the full
    // first char in that pathological case.
    if cut == 0 && !delta_bytes.is_empty() {
        cut = delta_bytes.len().min(4);
        while cut < delta_bytes.len() && incomplete_utf8_suffix_len(&delta_bytes[..cut]) > 0 {
            cut += 1;
        }
    }
    if cut == 0 {
        return None;
    }

    let delta = String::from_utf8_lossy(&delta_bytes[..cut]).into_owned();
    let consumed = cut as u64;

    *last_total = if gap {
        total - (delta_bytes.len() as u64 - consumed.min(delta_bytes.len() as u64))
    } else {
        *last_total + consumed
    };

    let payload = PartialResultPayload {
        delta,
        total_bytes: total,
        truncated,
        gap,
    };

    Some(ToolProgress::Custom {
        subkind: "partial_result".to_owned(),
        payload: serde_json::to_value(&payload).expect("PartialResultPayload always serializes"),
    })
}
