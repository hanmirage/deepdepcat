//! Client-side doom-loop detection for LLM streaming output.
//!
//! Detects repetitive generation patterns during streaming:
//! - **Tail repetition**: the same text unit repeats N times at the end of
//!   the accumulated buffer (e.g. `"done. done. done. done."`).
//!
//! When a confident signal is detected the actor can abort the stream,
//! inject a correction prompt, and resample with a higher temperature.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// Minimum repetitions before a tail-repetition signal is emitted.
const TAIL_REPETITION_THRESHOLD: usize = 4;
/// Shortest repeating unit (chars) worth flagging.
const MIN_UNIT_LEN: usize = 3;
/// Longest repeating unit (chars) — beyond this the text is likely normal prose.
const MAX_UNIT_LEN: usize = 500;
/// Sliding window — only the tail of the accumulated buffer is examined.
const CHECK_WINDOW: usize = 4096;

/// Kind of doom-loop signal detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoomLoopKind {
    /// The same text fragment repeats at the end of the output.
    TailRepetition,
}

/// A single doom-loop signal — the repeated unit, how many times it
/// repeated, and the kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoomLoopSignal {
    /// The text that repeated.
    pub repeated_unit: String,
    /// How many times it appeared consecutively.
    pub repetition_count: usize,
    /// Signal classification.
    pub kind: DoomLoopKind,
}

/// Streaming doom-loop detector — feed text chunks via [`push`](Self::push),
/// check for signals, and reset between requests.
#[derive(Debug)]
pub struct DoomLoopDetector {
    /// Rolling text buffer (only the tail `CHECK_WINDOW` chars are kept).
    buffer: String,
    /// Whether a signal has already fired (prevents duplicate triggers).
    triggered: bool,
    /// The last detected signal (available via [`take_signal`](Self::take_signal)).
    last_signal: Option<DoomLoopSignal>,
}

impl Default for DoomLoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl DoomLoopDetector {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            triggered: false,
            last_signal: None,
        }
    }

    /// Append a text chunk and run detection. Returns `Some(signal)` on the
    /// first detection; subsequent calls return `None` until a fresh
    /// detector is created (each request gets its own, see `parse_stream`).
    pub fn push(&mut self, chunk: &str) -> Option<DoomLoopSignal> {
        if self.triggered {
            return None;
        }
        self.buffer.push_str(chunk);
        self.trim_buffer();

        let signal = detect_tail_repetition(&self.buffer);
        if signal.is_some() {
            self.triggered = true;
            self.last_signal = signal.clone();
        }
        signal
    }

    /// Extract the last signal (if any) without clearing `triggered`.
    pub fn take_signal(&mut self) -> Option<DoomLoopSignal> {
        self.last_signal.take()
    }

    fn trim_buffer(&mut self) {
        let char_count = self.buffer.chars().count();
        if char_count > CHECK_WINDOW * 2 {
            let kept: String = self
                .buffer
                .chars()
                .rev()
                .take(CHECK_WINDOW)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            self.buffer = kept;
        }
    }
}

/// Detect tail-repetition in text: scan from the end, trying unit lengths
/// from `MIN_UNIT_LEN` up to `MAX_UNIT_LEN`, and return the first unit
/// that repeats ≥ `TAIL_REPETITION_THRESHOLD` times.
///
/// Implemented with a Z-function over the reversed tail window: `z[u]` is
/// the longest common prefix of the reversed window and itself shifted by
/// `u`, i.e. the number of chars that repeat block-aligned with period
/// `u`. A block-aligned run of ≥ `TAIL_REPETITION_THRESHOLD` units is
/// exactly `z[u] >= (threshold - 1) * u` — the same condition as the
/// naive per-unit scan, but O(window) per push instead of O(window × units)
/// with per-unit allocations (the doom detector runs on every streamed
/// text delta, i.e. at the highest streaming frequency).
pub fn detect_tail_repetition(text: &str) -> Option<DoomLoopSignal> {
    let reversed: Vec<char> = text.chars().rev().take(CHECK_WINDOW).collect();
    let len = reversed.len();
    if len < MIN_UNIT_LEN * TAIL_REPETITION_THRESHOLD {
        return None;
    }

    let z = z_function(&reversed);
    let max_unit = MAX_UNIT_LEN.min(len / TAIL_REPETITION_THRESHOLD);
    for unit_len in MIN_UNIT_LEN..=max_unit {
        if z[unit_len] < (TAIL_REPETITION_THRESHOLD - 1) * unit_len {
            continue;
        }
        // "aaaa…" units are noise, not a doom loop — skip and keep scanning
        // (only meaningful for unit_len ≥ MIN_UNIT_LEN > 1).
        let first = reversed[0];
        if reversed[..unit_len].iter().all(|c| *c == first) {
            continue;
        }
        let unit: String = reversed[..unit_len].iter().rev().collect();
        // Structural noise is NOT a doom loop: a repeated unit with no
        // alphanumeric characters is markdown table separators (" --- |"),
        // horizontal rules, or box-drawing — not looping prose/thinking.
        // A genuine loop always carries letters/digits/CJK ("done. done.",
        // "再次失败 再次失败"). Skip and keep scanning for a real unit.
        if !unit.chars().any(|c| c.is_alphanumeric()) {
            continue;
        }
        let repetition_count = z[unit_len] / unit_len + 1;
        return Some(DoomLoopSignal {
            repeated_unit: unit,
            repetition_count,
            kind: DoomLoopKind::TailRepetition,
        });
    }

    None
}

/// Z-array: `z[i]` = length of the longest prefix of `s` that is also a
/// prefix of `s[i..]` (linear time, no allocations beyond the result).
fn z_function(s: &[char]) -> Vec<usize> {
    let n = s.len();
    let mut z = vec![0usize; n];
    let (mut l, mut r) = (0usize, 0usize);
    for i in 1..n {
        if i < r {
            z[i] = (r - i).min(z[i - l]);
        }
        while i + z[i] < n && s[z[i]] == s[i + z[i]] {
            z[i] += 1;
        }
        if i + z[i] > r {
            l = i;
            r = i + z[i];
        }
    }
    z
}

/// Build a correction prompt injected into the conversation to break the loop.
pub fn recovery_prompt(signal: &DoomLoopSignal) -> String {
    format!(
        "[Output loop detected] Your previous output repeated \"{}\" {} times. \
         Stop repeating and provide the next actionable step or final answer.",
        safe_truncate(&signal.repeated_unit, 100),
        signal.repetition_count,
    )
}

fn safe_truncate(s: &str, max: usize) -> Cow<'_, str> {
    if s.chars().count() <= max {
        Cow::Borrowed(s)
    } else {
        let truncated: String = s.chars().take(max).collect();
        Cow::Owned(format!("{}...", truncated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_repetition_returns_none() {
        assert!(detect_tail_repetition("hello world this is normal text").is_none());
        assert!(detect_tail_repetition("short").is_none());
        assert!(detect_tail_repetition("").is_none());
    }

    #[test]
    fn markdown_table_separator_is_not_a_doom_loop() {
        // A GFM table with 4+ columns emits "| --- | --- | --- | --- |" —
        // the unit " --- |" repeats 4x with no alphanumerics. This is
        // structural markdown, NOT looping prose; it must not abort a
        // legitimate generation mid-table.
        let table = "| a | b | c | d |\n| --- | --- | --- | --- |\n| 1 | 2 | 3 | 4 |";
        assert!(
            detect_tail_repetition(table).is_none(),
            "markdown table separator must not fire the doom detector"
        );
        let mut detector = DoomLoopDetector::new();
        assert!(
            detector.push(table).is_none(),
            "a full table with separators must stream through cleanly"
        );
    }

    #[test]
    fn detects_simple_tail_repetition() {
        let signal = detect_tail_repetition("prefix done. done. done. done.").unwrap();
        assert_eq!(signal.kind, DoomLoopKind::TailRepetition);
        assert!(signal.repetition_count >= 4);
        assert!(signal.repeated_unit.contains("done"));
    }

    #[test]
    fn ignores_single_char_repetition() {
        assert!(detect_tail_repetition("aaaaaaaaaaaaaaaa").is_none());
    }

    #[test]
    fn threshold_boundary() {
        assert!(detect_tail_repetition("go. go. go. ").is_none());
        assert!(detect_tail_repetition("go. go. go. go. ").is_some());
    }

    #[test]
    fn detector_accumulates_across_chunks() {
        let mut detector = DoomLoopDetector::new();
        assert!(detector.push("prefix ").is_none());
        assert!(detector.push("done. ").is_none());
        assert!(detector.push("done. ").is_none());
        assert!(detector.push("done. ").is_none());
        let signal = detector.push("done. ").unwrap();
        assert!(signal.repetition_count >= 4);
    }

    #[test]
    fn recovery_prompt_contains_info() {
        let signal = DoomLoopSignal {
            repeated_unit: "loop text ".to_string(),
            repetition_count: 5,
            kind: DoomLoopKind::TailRepetition,
        };
        let prompt = recovery_prompt(&signal);
        assert!(prompt.contains("loop text"));
        assert!(prompt.contains("5"));
    }
}
