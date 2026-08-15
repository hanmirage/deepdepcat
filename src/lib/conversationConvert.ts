/**
 * Conversation item conversion — transforms backend `ConversationItem[]`
 * (as serialized JSON from `get_session_messages`) into frontend `UIMessage[]`
 * for display in the chat UI.
 *
 * Backend ConversationItem is tagged with `#[serde(tag = "role", rename_all = "snake_case")]`:
 * - `{ role: "system", content: string }`
 * - `{ role: "user", content: ContentPart[] }`
 * - `{ role: "assistant", content: string, tool_calls?: ToolCall[], model?: string, usage?: TokenUsage, reasoning_content?: string }`
 * - `{ role: "tool_result", tool_call_id: string, content: string, is_error: boolean }`
 * - `{ role: "reasoning", content: string }`
 *
 * Frontend UIMessage groups these into user/assistant messages with blocks.
 * Tool results are attached to the preceding assistant message's tool_call block.
 */

import type { UIMessage, MessageBlock, ToolCallState } from "@/types";

/** Raw backend ConversationItem shapes (as received from invoke). */
interface RawConversationItem {
  role: string;
  content?: unknown;
  tool_calls?: RawToolCall[];
  model?: string;
  usage?: { prompt_tokens?: number; completion_tokens?: number };
  reasoning_content?: string;
  tool_call_id?: string;
  is_error?: boolean;
}

interface RawToolCall {
  id: string;
  name: string;
  arguments: string;
}

interface RawContentPart {
  text?: string;
}

/**
 * Convert backend ConversationItem[] to frontend UIMessage[].
 *
 * The backend splits ONE agent turn into multiple assistant items — one per
 * tool loop (text before a tool batch, then text after). The streaming path
 * merges all of those into a single UIMessage (multi-block). This converter
 * must do the same on restore: all consecutive assistant items between two
 * user messages fold into ONE UIMessage, so a restored turn looks exactly
 * like it did while streaming (no per-loop message row with its own action
 * bar).
 *
 * - System and standalone Reasoning items are skipped (not shown in chat UI).
 * - ToolResult items are merged into the preceding assistant message's
 *   matching tool_call block as `result`.
 */
export function conversationItemsToUIMessages(
  items: unknown[],
): UIMessage[] {
  const rawItems = items as RawConversationItem[];
  const messages: UIMessage[] = [];
  let msgCounter = 0;
  /** The assistant UIMessage being accumulated for the current turn. */
  let pendingAssistant: UIMessage | null = null;

  // Push the pending assistant (if any) and start a fresh user message.
  const flushAssistant = () => {
    if (pendingAssistant) {
      messages.push(pendingAssistant);
      pendingAssistant = null;
    }
  };

  for (const item of rawItems) {
    switch (item.role) {
      case "user": {
        const text = extractUserText(item.content);
        // A user message closes the previous turn — flush its assistant.
        flushAssistant();
        messages.push({
          id: `restored-${msgCounter++}`,
          role: "user",
          blocks: [{ type: "text", content: text }],
          timestamp: Date.now(),
        });
        break;
      }

      case "assistant": {
        // One turn = one UIMessage. If an assistant message is already being
        // accumulated (a previous tool loop of the same turn), append this
        // loop's text / reasoning / tool calls to it instead of starting a
        // new row — mirrors the streaming merge.
        if (!pendingAssistant) {
          pendingAssistant = {
            id: `restored-${msgCounter++}`,
            role: "assistant",
            blocks: [],
            timestamp: Date.now(),
          };
        }

        if (item.reasoning_content) {
          pendingAssistant.blocks.push({
            type: "reasoning",
            content: item.reasoning_content,
          });
        }

        if (typeof item.content === "string" && item.content.length > 0) {
          pendingAssistant.blocks.push({ type: "text", content: item.content });
        }

        const toolCalls: ToolCallState[] = (item.tool_calls ?? []).map((tc) => ({
          id: tc.id,
          name: tc.name,
          arguments: tc.arguments,
          status: "done" as const,
        }));

        for (const tc of toolCalls) {
          pendingAssistant.blocks.push({ type: "tool_call", tool: tc });
        }

        if (item.model) pendingAssistant.model = item.model;
        if (item.usage) {
          pendingAssistant.tokenUsage = {
            prompt: item.usage.prompt_tokens ?? 0,
            completion: item.usage.completion_tokens ?? 0,
          };
        }
        break;
      }

      case "tool_result": {
        // Attach to the matching tool_call in the pending assistant (the
        // tool loop that produced it — same turn, same UIMessage).
        if (pendingAssistant && item.tool_call_id) {
          for (const block of pendingAssistant.blocks) {
            if (block.type === "tool_call" && block.tool.id === item.tool_call_id) {
              block.tool.result = typeof item.content === "string" ? item.content : "";
              block.tool.status = item.is_error ? "error" : "done";
              break;
            }
          }
        }
        break;
      }

      // system and reasoning items are not displayed in the chat UI
      default:
        break;
    }
  }

  // Flush the trailing assistant turn.
  flushAssistant();

  return messages;
}

/** Extract plain text from a User message's ContentPart array. */
function extractUserText(content: unknown): string {
  if (typeof content === "string") return stripEnvironmentContext(content);
  if (Array.isArray(content)) {
    return content
      .map((part) => (typeof part === "object" && part !== null && "text" in part ? String((part as RawContentPart).text ?? "") : ""))
      .join("\n");
  }
  return "";
}

/**
 * Strip the <environment-context> block that older sessions persisted as part
 * of the user message (workspace path, git status, current time). The backend
 * no longer stores it, but old history still has it — hide it on restore.
 */
function stripEnvironmentContext(text: string): string {
  if (!text.startsWith("<environment-context>")) return text;
  const end = text.indexOf("</environment-context>");
  if (end === -1) return text;
  return text.slice(end + "</environment-context>".length).replace(/^\n+/, "");
}
