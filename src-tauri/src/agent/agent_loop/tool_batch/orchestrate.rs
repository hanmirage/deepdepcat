//! Batch tool orchestration — partition into order-preserving blocks, then
//! dispatch each block and aggregate permission denials.

use super::super::AgentLoop;
use crate::core::error::AppResult;
use crate::core::types::ToolCall;
use crate::hooks::{HookContext, HookEvent};
use tauri::Manager;
use tokio_util::sync::CancellationToken;

/// Partition tool calls into order-preserving blocks: consecutive calls with
/// the same concurrency safety merge into one block, then blocks run in
/// order. The OLD approach ran ALL parallel-safe calls before ALL serial
/// ones, dropping cross-group relative order — a model that emits
/// `edit_file` (serial) then `read_file` (parallel) to verify the edit had
/// the read execute FIRST and saw stale content.
fn partition_into_blocks<'a>(
    tool_calls: &'a [ToolCall],
    is_safe: impl Fn(&str) -> bool,
) -> Vec<Vec<&'a ToolCall>> {
    let mut blocks: Vec<Vec<&'a ToolCall>> = Vec::new();
    for tc in tool_calls {
        let safe = is_safe(&tc.name);
        if let Some(last) = blocks.last_mut() {
            if is_safe(&last[0].name) == safe {
                last.push(tc);
                continue;
            }
        }
        blocks.push(vec![tc]);
    }
    blocks
}

impl AgentLoop {
    /// Execute a batch of tool calls with PreToolUse/PostToolUse hooks.
    ///
    /// Returns the number of permission denials encountered. Concurrency-safe
    /// calls run together (`execute_parallel_group`); side-effecting calls
    /// run one at a time (`execute_serial_group`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_tool_batch(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        turn_id: &str,
        turn: u32,
        chat_state: &mut crate::agent::chat_state::ChatState,
        tool_calls: &[ToolCall],
        cancellation_token: &CancellationToken,
        debug_mode: bool,
        skill_engine: Option<&crate::skills::activation::SkillActivationEngine>,
    ) -> AppResult<u32> {
        // Whether the main model natively accepts images. Text-only models
        // (DeepSeek) reject image blocks with HTTP 400, so embedding is
        // skipped for them — their pictures are transcribed to text by the
        // image_transcribe pipeline instead.
        let can_see_images = {
            let state = app.state::<crate::bootstrap::AppState>();
            let sessions = state.sessions.lock().await;
            sessions.model_catalog().supports_vision(&chat_state.model)
        };

        // Partition into order-preserving blocks: consecutive same-safety
        // calls merge, blocks run in order. A parallel-safe block runs
        // together (`join_all`), a side-effecting block runs one at a time —
        // but the model's cross-group relative order is preserved.
        let blocks = partition_into_blocks(tool_calls, |name| {
            self.tool_dispatcher.is_concurrency_safe(name)
        });

        let mut permission_denials: u32 = 0;
        for block in &blocks {
            if self.tool_dispatcher.is_concurrency_safe(&block[0].name) {
                permission_denials += self
                    .execute_parallel_group(
                        app,
                        session_id,
                        turn_id,
                        turn,
                        chat_state,
                        block,
                        cancellation_token,
                        debug_mode,
                        skill_engine,
                        can_see_images,
                    )
                    .await?;
            } else {
                permission_denials += self
                    .execute_serial_group(
                        app,
                        session_id,
                        turn_id,
                        turn,
                        chat_state,
                        block,
                        cancellation_token,
                        debug_mode,
                        skill_engine,
                        can_see_images,
                    )
                    .await?;
            }
        }

        // PostToolBatch hook — the whole batch resolved (observe-only).
        let batch_ctx = HookContext::new(HookEvent::PostToolBatch, session_id)
            .with_data("permission_denials", serde_json::json!(permission_denials));
        self.hook_executor.execute_observe(&batch_ctx).await;
        Ok(permission_denials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: name.to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
        }
    }

    /// read_file / grep are concurrency-safe (parallel); edit_file is not.
    fn is_safe(name: &str) -> bool {
        matches!(name, "read_file" | "grep")
    }

    #[test]
    fn partition_preserves_cross_group_order() {
        // Model emits edit (serial) THEN read (parallel) to verify the edit.
        // The blocks must keep that order — the old "all parallel first"
        // behavior ran the read first and saw stale content.
        let calls = vec![call("edit_file"), call("read_file")];
        let blocks = partition_into_blocks(&calls, is_safe);
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(blocks[0][0].name, "edit_file");
        assert_eq!(blocks[1][0].name, "read_file");
    }

    #[test]
    fn partition_merges_adjacent_same_safety() {
        let calls = vec![call("read_file"), call("grep"), call("edit_file")];
        let blocks = partition_into_blocks(&calls, is_safe);
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(blocks[0].len(), 2); // read + grep run together
        assert_eq!(blocks[1].len(), 1); // edit serial
    }

    #[test]
    fn partition_keeps_safe_calls_separated_by_serial_apart() {
        // read / edit / grep — the two parallel-safe calls are NOT merged
        // because the serial edit sits between them (order wins over
        // parallelism).
        let calls = vec![call("read_file"), call("edit_file"), call("grep")];
        let blocks = partition_into_blocks(&calls, is_safe);
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert_eq!(blocks[0][0].name, "read_file");
        assert_eq!(blocks[1][0].name, "edit_file");
        assert_eq!(blocks[2][0].name, "grep");
    }

    #[test]
    fn partition_handles_empty_input() {
        assert!(partition_into_blocks(&[], is_safe).is_empty());
    }
}
