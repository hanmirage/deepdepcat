/**
 * Depwork (document workspace) types — academic & office document processing.
 *
 * #79 merged the chat message type system into `@/types/chat` (the depwork
 * message/block/tool-call/chip types were structural duplicates of the code
 * variants). The names below are kept as COMPATIBILITY ALIASES so existing
 * components keep compiling; new code should use the `@/types/chat` names
 * directly.
 */

import type {
  ContextChip,
  InteractionMode,
  MessageBlock,
  ProgressKind,
  ToolCallState,
  UIMessage,
} from "@/types/chat";

// Re-export from tauri.ts for backward compatibility
export type { DepworkTask } from "@/lib/tauri";

/** Progress kind for document processing tools (alias of the shared one). */
export type DepworkProgressKind = ProgressKind;

/** State of a single tool call within a depwork message. */
export type DepworkToolCallState = ToolCallState;

/** A single block within a depwork message. */
export type DepworkMessageBlock = MessageBlock;

/** UI message type for depwork conversations. */
export type DepworkMessage = UIMessage;

/** Context chip attached to depwork input — documents, papers, URLs. */
export type DepworkContextChip = ContextChip;

/** Interaction mode for depwork — how the agent handles document operations. */
export type DepworkInteractionMode = InteractionMode;

/** Document type for academic/office files. */
export type DocumentType =
  | "pdf"
  | "doc"
  | "docx"
  | "txt"
  | "md"
  | "xlsx"
  | "csv"
  | "pptx"
  | "unknown";

/** Document metadata for the workspace. */
export interface DepworkDocument {
  id: string;
  name: string;
  path: string;
  type: DocumentType;
  size: number | null;
  lastModified: number;
}
