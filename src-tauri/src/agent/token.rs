//! Token estimation — bytes/4 for ASCII, character-based for CJK.
//!
//! A fast approximation that doesn't require loading a BPE tokenizer. CJK
//! glyphs tokenize at roughly one token each (not the ~0.75 that bytes/4
//! implies), so they are counted per character; the rest is ASCII/bytes.
//! The estimate is conservative (over-counts rather than under-counts),
//! which is safer for context-window management.

use crate::core::types::{ContentPart, ConversationItem, ToolDefinition};

/// Approximate bytes per token (industry-standard heuristic: 4 bytes ≈ 1 token).
pub const BYTES_PER_TOKEN: usize = 4;

/// Approximate tokens per image (based on typical model pricing).
pub const IMAGE_TOKEN_ESTIMATE: u64 = 765;

/// Estimate the number of tokens in a text string.
///
/// CJK characters are counted at roughly one token each (a Chinese glyph is
/// ~1 BPE token), while the rest is ASCII/bytes (~4 bytes/token). The old
/// bytes/4-only heuristic under-counted CJK by ~25-30% (3 bytes/char →
/// ~0.75 tokens/char), pushing compaction past the real window for the
/// product's primary (Chinese) content. The Latin remainder uses ceiling
/// division so a short string ("ok", "[]", "exit 0") never estimates to 0.
pub fn estimate_text_tokens(text: &str) -> u64 {
    let mut cjk = 0u64;
    let mut latin_bytes = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            latin_bytes += c.len_utf8();
        }
    }
    cjk + (latin_bytes as u64).div_ceil(BYTES_PER_TOKEN as u64)
}

/// Whether a character tokenizes as a CJK glyph (roughly one BPE token each
/// on DeepSeek-class tokenizers), as opposed to ASCII (~4 bytes/token).
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
            | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
            | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
            | '\u{3040}'..='\u{30FF}' // Hiragana + Katakana
            | '\u{3000}'..='\u{303F}' // CJK punctuation
            | '\u{FF00}'..='\u{FFEF}' // Fullwidth forms
    )
}

/// Estimate the token count for images.
pub fn estimate_image_tokens(num_images: u64) -> u64 {
    num_images * IMAGE_TOKEN_ESTIMATE
}

/// Estimate tokens for a single conversation item.
pub fn estimate_item_tokens(item: &ConversationItem) -> u64 {
    match item {
        ConversationItem::System(s) => estimate_text_tokens(&s.content),
        ConversationItem::User(u) => {
            let mut tokens = 0u64;
            let mut images = 0u64;
            for p in &u.content {
                match p {
                    ContentPart::Text { text } => tokens += estimate_text_tokens(text),
                    ContentPart::Image { .. } => images += 1,
                }
            }
            tokens + estimate_image_tokens(images)
        }
        ConversationItem::Assistant(a) => {
            let mut tokens = estimate_text_tokens(&a.content);
            for tc in &a.tool_calls {
                tokens += estimate_text_tokens(&tc.name);
                tokens += estimate_text_tokens(&tc.arguments);
            }
            if let Some(reasoning) = &a.reasoning_content {
                tokens += estimate_text_tokens(reasoning);
            }
            tokens
        }
        ConversationItem::ToolResult(tr) => estimate_text_tokens(&tr.content),
        ConversationItem::Reasoning(r) => {
            let mut tokens = estimate_text_tokens(&r.content);
            if let Some(enc) = &r.encrypted_content {
                tokens += estimate_text_tokens(enc);
            }
            tokens
        }
    }
}

/// Estimate the total token count for a conversation.
pub fn estimate_conversation_tokens(items: &[ConversationItem]) -> u64 {
    items.iter().map(estimate_item_tokens).sum()
}

/// Split a conversation's estimate into conversation content vs tool results
/// (tool outputs are tracked separately in the usage breakdown).
pub fn estimate_conversation_tokens_by_kind(items: &[ConversationItem]) -> (u64, u64) {
    let mut conv = 0u64;
    let mut tool = 0u64;
    for item in items {
        match item {
            ConversationItem::ToolResult(tr) => tool += estimate_text_tokens(&tr.content),
            _ => conv += estimate_item_tokens(item),
        }
    }
    (conv, tool)
}

/// Estimate the token cost of a single tool definition (name + description + parameters).
pub fn estimate_tool_definition_tokens(td: &ToolDefinition) -> u64 {
    let mut tokens = estimate_text_tokens(&td.function.name);
    if let Some(desc) = &td.function.description {
        tokens += estimate_text_tokens(desc);
    }
    tokens += estimate_text_tokens(&td.function.parameters.to_string());
    tokens
}

/// Estimate the total token cost of all tool definitions.
pub fn estimate_tool_definitions_tokens(tds: &[ToolDefinition]) -> u64 {
    tds.iter().map(estimate_tool_definition_tokens).sum()
}

/// Estimate the system prompt token cost.
pub fn estimate_system_prompt_tokens(system_prompt: &str) -> u64 {
    estimate_text_tokens(system_prompt)
}

/// Calculate the total estimated token usage for an API request:
/// system prompt + conversation + tool definitions + overhead.
pub fn estimate_request_tokens(
    system_prompt: &str,
    conversation: &[ConversationItem],
    tools: &[ToolDefinition],
) -> u64 {
    let system = estimate_system_prompt_tokens(system_prompt);
    let messages = estimate_conversation_tokens(conversation);
    let tool_defs = estimate_tool_definitions_tokens(tools);
    // Add ~100 tokens overhead for message framing
    system + messages + tool_defs + 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_is_counted_per_character_not_bytes_div_four() {
        // 6 Chinese chars = 18 bytes → bytes/4 would give 4; real estimate is ~6.
        let cjk = estimate_text_tokens("你好世界这是");
        assert!(cjk >= 6, "CJK must not under-count: got {cjk}");
        // A mixed string counts CJK at ~1/char plus ASCII at bytes/4.
        let mixed = estimate_text_tokens("你好 hello world");
        assert!(mixed >= 5, "mixed CJK+latin must not under-count: got {mixed}");
    }

    #[test]
    fn short_strings_never_floor_to_zero() {
        // The old bytes/4 estimate gave 0 for <4-byte strings.
        assert!(estimate_text_tokens("ok") > 0);
        assert!(estimate_text_tokens("[]") > 0);
        assert!(estimate_text_tokens("exit 0") > 0);
        assert!(estimate_text_tokens("") == 0);
    }
}
