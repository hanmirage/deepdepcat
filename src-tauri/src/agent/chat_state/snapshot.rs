//! State snapshot, restore, and rewind support — enables fork/rewind
//! by capturing the full `ChatState` and restoring from a snapshot.
//!
//! Also provides fire-and-forget usage tracking methods that avoid
//! oneshot channel overhead for high-frequency token accounting.

use crate::core::types::ConversationItem;

use super::ChatState;

impl ChatState {
    /// Truncate conversation to keep only messages up to `target` prompt index.
    ///
    /// After truncation, `prompt_index` is set to `target`, `turn_capture`
    /// is cleared, and token estimates are recomputed.
    pub fn truncate_to_prompt_index(&mut self, target: usize) {
        if target == 0 {
            self.conversation.clear();
        } else {
            let mut user_count = 0;
            let mut truncate_at = self.conversation.len();
            for (i, item) in self.conversation.iter().enumerate() {
                if matches!(item, ConversationItem::User(_)) {
                    user_count += 1;
                    if user_count > target {
                        truncate_at = i;
                        break;
                    }
                }
            }
            self.conversation.truncate(truncate_at);
        }
        // History shrank — the persisted tail is now stale; rewrite on next
        // persist instead of appending from a bogus checkpoint.
        self.persisted_upto = 0;
        self.prompt_index = target;
        // prompt_texts must track prompt_index (each user turn's first text
        // part) — leaving it untruncated desynced the two, so a later
        // truncate_from_user_message would rebuild from a stale list.
        self.prompt_texts.truncate(target);
        self.turn_capture = None;
        self.check_invariants();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_prompt_index_removes_later_messages() {
        let mut state = ChatState::new("test-model", 128_000);
        state.push_user_message("first");
        state.push_assistant_message("reply1", vec![], None, None);
        state.push_user_message("second");
        state.push_assistant_message("reply2", vec![], None, None);
        assert_eq!(state.prompt_index, 2);

        state.truncate_to_prompt_index(1);
        assert_eq!(state.prompt_index, 1);
        assert_eq!(state.conversation.len(), 2);
        assert!(
            matches!(&state.conversation[1], ConversationItem::Assistant(a) if a.content == "reply1")
        );
    }

    #[test]
    fn truncate_to_zero_clears_all() {
        let mut state = ChatState::new("test-model", 128_000);
        state.push_user_message("hello");
        state.push_assistant_message("hi", vec![], None, None);

        state.truncate_to_prompt_index(0);
        assert_eq!(state.prompt_index, 0);
        assert!(state.conversation.is_empty());
    }

    #[test]
    fn truncate_to_prompt_index_truncates_prompt_texts_too() {
        let mut state = ChatState::new("test-model", 128_000);
        state.push_user_message("first");
        state.push_assistant_message("reply1", vec![], None, None);
        state.push_user_message("second");
        state.push_assistant_message("reply2", vec![], None, None);
        assert_eq!(state.prompt_texts.len(), 2);

        // prompt_texts must track prompt_index — leaving it untruncated
        // desynced the two (a later truncate_from_user_message would rebuild
        // from the stale list).
        state.truncate_to_prompt_index(1);
        assert_eq!(state.prompt_index, 1);
        assert_eq!(state.prompt_texts.len(), 1);
        assert_eq!(state.prompt_texts[0], "first");
    }
}
