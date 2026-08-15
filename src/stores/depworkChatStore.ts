/**
 * Depwork chat store — a "depwork" work-mode instance of the shared chat
 * store factory (see `src/stores/chatStore.ts`).
 *
 * #79 merged the two hand-copied chat store twins (~2800 lines) into one
 * `createChatStore(workMode)` implementation; this module now only pins the
 * depwork instance and re-exports the API surface kept for backward
 * compatibility (components and tests import from this path).
 */

import { createChatStore, extractDocumentPath, basenameOf } from "@/stores/chatStore";
import type { ChatState } from "@/stores/chatStore";

/** Depwork-mode chat store. */
export const useDepworkChatStore = createChatStore("depwork");

/** Backward-compatible alias of the shared store interface. */
export type DepworkChatState = ChatState;

// Pure helpers kept exported for components/tests (document dispatch).
export { extractDocumentPath, basenameOf };

// Backward-compatible type aliases (see @/types/depwork).
export type {
  DepworkMessage,
  DepworkMessageBlock,
  DepworkToolCallState,
  DepworkProgressKind,
  DepworkContextChip,
  DepworkInteractionMode,
} from "@/types/depwork";
