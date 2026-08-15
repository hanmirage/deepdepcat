/**
 * Chat store — split by concern (types / prefs / mode detection / stream state).
 */

import type { ModelWithPricing } from "@/config/models";
import type { ReasoningMode } from "@/components/chat/ReasoningSelector";
import type {
  UIMessage,
  ModelInfo,
  InteractionMode,
  ContextChip,
  AgentMode,
  SubagentUIRecord,
  StreamPhase,
} from "@/types";
import type { UserAskRequest } from "@/lib/tauri";
export type ChatWorkMode = "code" | "depwork";
export interface CompactionRecord {
  tokens: number;
  summary: string;
  at: number;
}

export interface ChatState {
  messages: UIMessage[];
  models: ModelWithPricing[];
  selectedModel: ModelWithPricing | null;
  currentSessionId: string | null;
  /** Current session's title — tracks auto-generated titles so the sidebar
   *  stays in sync without re-fetching the session. */
  sessionTitle: string;
  /** True while a selected session's history is being fetched (skeleton UI). */
  sessionLoading: boolean;
  isStreaming: boolean;
  /** True while the running turn is paused (backend watch flipped; the loop
   *  is parked at its checkpoint, not cancelled). Current-session view. */
  isPaused: boolean;
  /** Live phase of the current session's turn (connecting → thinking →
   *  generating / tool_running → idle). Drives the StreamStatusLine. */
  streamPhase: StreamPhase;
  /** Latency (ms) from turn_start to the first streamed token — the model's
   *  cold-start / thinking-before-output cost. Set once per turn on the first
   *  reasoning/text delta; cleared at turn end. Drives the streaming TTFT readout. */
  firstTokenLatencyMs: number | null;
  inputText: string;
  /** Text queued to send automatically when the current stream ends.
   *  Current-session view — per-session queue lives in the stream states. */
  queuedText: string | null;
  notification: string | null;
  /** Recent compaction events (context panel history). */
  compactions: CompactionRecord[];
  /** Memory auto-injected into the current turn (memory_injected event) —
   *  drives the "已引用记忆" marker; cleared at turn end. */
  memoryRef: { count: number; snippet: string } | null;
  totalTokens: { prompt: number; completion: number; cacheHit: number; cacheMiss: number; cachedRead: number; reasoning: number };

  /** Pending ask_user request from the agent (null when none). */
  pendingAskUser: UserAskRequest | null;

  /** Pending MCP elicitation request (null when none). */
  pendingElicitation: {
    elicitationId: string;
    serverName: string;
    message: string;
  } | null;

  /** Live subagent state, keyed by subagent_id (event-driven; the activity
   *  panel additionally polls list_active_workers for a full picture). */
  subagents: Record<string, SubagentUIRecord>;

  // ── Context chips ──────────────────────────────────────────
  contextChips: ContextChip[];

  // ── Interaction mode ───────────────────────────────────────
  interactionMode: InteractionMode;
  /** True when the user manually selected a mode — auto-detection won't override. */
  manualModeOverride: boolean;

  // ── Reasoning mode (DeepSeek) ──────────────────────────────
  reasoningMode: ReasoningMode;

  /** Execution mode — the backend AgentLoopMode (standard / plan_execute /
   *  reflexion / coordinator). Distinct from the interaction mode
   *  (permission behavior): this drives the agent LOOP strategy. Standard
   *  is the default; plan_execute is implied by interactionMode === "read_only". */
  agentMode: AgentMode;
  /** Set the execution mode (persisted). */
  setAgentMode: (mode: AgentMode) => void;

  /** Custom agent persona for this surface ("" = default). Sent with every
   *  message; the backend resolves it per session. */
  selectedAgent: string;
  /** Set the custom agent persona (persisted per mode). */
  setSelectedAgent: (name: string) => void;

  // ── Actions ────────────────────────────────────────────────
  setInputText: (text: string) => void;
  setSelectedModel: (model: ModelInfo) => void;
  loadModels: () => Promise<void>;
  ensureSession: () => Promise<string>;
  /** Send the current input. When streaming, `whenBusy` decides: "queue" waits,
   *  "interrupt" cancels the running turn first. Default: queue. */
  sendMessage: (whenBusy?: "queue" | "interrupt") => Promise<void>;
  stopStreaming: () => Promise<void>;
  /** Pause the running turn (backend suspends the loop at its checkpoint).
   *  No-op when not streaming or already paused. */
  pauseStreaming: () => Promise<void>;
  /** Resume a paused turn. No-op when not paused. */
  resumeStreaming: () => Promise<void>;
  /** Cancel a queued send and restore the text into the input box. */
  clearQueuedText: () => void;
  clearMessages: () => void;
  dismissNotification: () => void;
  /** Recall (delete) a message and everything after it. */
  deleteMessage: (id: string) => Promise<void>;
  /** Whether the given session currently has an active stream. Multi-session
   *  aware — used by the sidebar to show per-session live indicators. */
  isSessionStreaming: (id: string) => boolean;
  /** Stop a SPECIFIC session's stream (not just the current one). */
  stopSessionStreaming: (sessionId: string) => Promise<void>;
  /** Drop a session's stream state entirely (listener + queue + counters).
   *  Called when the session is deleted — stops any live turn first so late
   *  events can't write into a removed conversation, and the map entry
   *  stops leaking. */
  disposeSession: (sessionId: string) => void;

  // ── Session restore (called by useSessionRestore) ────────
  setSessionId: (id: string | null) => void;
  setSessionTitle: (title: string) => void;
  setMessages: (messages: UIMessage[]) => void;
  setSessionLoading: (loading: boolean) => void;

  // ── Context chip actions ───────────────────────────────────
  addContextChip: (chip: ContextChip) => void;
  removeContextChip: (id: string) => void;
  clearContextChips: () => void;
  addFileContext: () => Promise<void>;
  addFolderContext: () => Promise<void>;
  addUrlContext: (url: string) => void;
  /** Attach a depwork "paper" chip (a document the agent opened). */
  addPaperContext: (title: string) => void;

  // ── Interaction mode actions ───────────────────────────────
  setInteractionMode: (mode: InteractionMode) => void;
  // ── Reasoning mode action ──────────────────────────────────
  setReasoningMode: (mode: ReasoningMode) => void;
  /** Internal: set mode without marking manual override (for auto-detection). */
  _autoSetMode: (mode: InteractionMode) => void;

  // ── Ask user ──────────────────────────────────────────────
  setPendingAskUser: (req: UserAskRequest | null) => void;
  respondAskUser: (response: string) => Promise<void>;

  // ── MCP elicitation ───────────────────────────────────────
  setPendingElicitation: (req: {
    elicitationId: string;
    serverName: string;
    message: string;
  } | null) => void;
  respondElicitation: (
    elicitationId: string,
    action: "accept" | "decline" | "cancel",
    content?: unknown,
  ) => Promise<void>;
}
