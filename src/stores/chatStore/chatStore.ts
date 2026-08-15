/**
 * Chat store — split by concern (types / prefs / mode detection / stream state).
 */

import { create } from "zustand";
import { logWarn } from "@/lib/logger";
import type { InteractionMode, AgentMode, ContextChip } from "@/types";
import type { ReasoningMode } from "@/components/chat/ReasoningSelector";
import type { ModelWithPricing } from "@/config/models";
import { sessionApi, systemApi, pickFiles, pickFolders, askUserApi, elicitationApi } from "@/lib/tauri";
import { useAppStore } from "@/stores/appStore";
import {
  useSettingsStore,
  buildModelsFromProviders,
  resolveContextWindow,
} from "@/stores/settingsStore";
import {
  loadPref,
  savePref,
  PREF_MODEL,
  PREF_MODE,
  PREF_REASONING,
  PREF_AGENT_MODE,
  PREF_AGENT_PERSONA,
  DEPWORK_PREF_MODEL,
  DEPWORK_PREF_MODE,
  DEPWORK_PREF_AGENT_PERSONA,
} from "./prefs";
import { streamStates, streamState, sessionStreaming, syncStreamingBus } from "./streamState";
import { createSendMessage } from "./actions/sendMessage";
import { updateSessionMessages } from "./sessionMessages";
import { normalizeInteractionMode } from "./modeDetect";
import type { ChatState, ChatWorkMode } from "./types";

export function createChatStore(mode: ChatWorkMode) {
  // Per-mode persisted preference keys — the two surfaces keep independent
  // model/mode selections (a paper-crunching session must not overwrite the
  // code session's chosen model).
  const modelPrefKey = mode === "depwork" ? DEPWORK_PREF_MODEL : PREF_MODEL;
  const modePrefKey = mode === "depwork" ? DEPWORK_PREF_MODE : PREF_MODE;
  const personaPrefKey =
    mode === "depwork" ? DEPWORK_PREF_AGENT_PERSONA : PREF_AGENT_PERSONA;
  const store = create<ChatState>((set, get) => ({
  // ── Initial state ───────────────────────────────────────────
  messages: [],
  models: [],
  selectedModel: null,
  currentSessionId: null,
  sessionTitle: "New Session",
  sessionLoading: false,
  isStreaming: false,
  isPaused: false,
  streamPhase: "idle",
  firstTokenLatencyMs: null,
  inputText: "",
  queuedText: null,
  notification: null,
  compactions: [],
  memoryRef: null,
  totalTokens: { prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 },

  // ── Context chips initial ───────────────────────────────────
  contextChips: [],

  // ── Interaction mode initial ────────────────────────────────
  // Defaults to accept_edits (edits auto-approved, dangerous ops prompt —
  // the "normal" posture). A stored legacy value is normalized on load so
  // pre-collapse modes (plan/confirm/chat_only/auto) migrate cleanly.
  interactionMode: normalizeInteractionMode(loadPref(modePrefKey)),
  // A mode restored from localStorage was the user's manual choice — mark it
  // so auto-detection doesn't immediately overwrite it on the first message.
  manualModeOverride: loadPref(modePrefKey) !== null,

  pendingAskUser: null,
  pendingElicitation: null,
  subagents: {},

  // ── Reasoning mode initial ──────────────────────────────────
  reasoningMode: (loadPref(PREF_REASONING) as ReasoningMode | null) ?? "auto",

  // ── Execution mode initial ─────────────────────────────────
  agentMode: (loadPref(PREF_AGENT_MODE) as AgentMode | null) ?? "standard",
  setAgentMode: (mode) => {
    set({ agentMode: mode });
    savePref(PREF_AGENT_MODE, mode);
  },

  // ── Custom agent persona initial (per surface) ──────────────
  selectedAgent: loadPref(personaPrefKey) ?? "",
  setSelectedAgent: (name) => {
    set({ selectedAgent: name });
    savePref(personaPrefKey, name);
  },

  // ── Actions ────────────────────────────────────────────────
  setInputText: (text) => set({ inputText: text }),

  clearQueuedText: () =>
    set((s) => {
      // Clear the queue of the CURRENT session (the queued text shown in the
      // input chip) and restore the text into the input box.
      if (s.currentSessionId) {
        const st = streamStates.get(s.currentSessionId);
        if (st) st.queuedText = null;
      }
      return { queuedText: null, inputText: s.queuedText ?? s.inputText };
    }),

  setSelectedModel: (model) => {
    set({ selectedModel: model as ModelWithPricing });
    savePref(modelPrefKey, model.id);
  },

  loadModels: async () => {
    // Ensure settings store is initialized — appStore.initSystem also calls
    // this, but loadModels may run first (e.g. browser dev mode).
    await useSettingsStore.getState().init();

    // Single source of truth: the user's configured providers. The picker
    // mirrors Settings → Model Providers exactly — nothing hardcoded.
    // Auto-fetch models for providers that have an API key but an empty
    // model list (saves a manual trip to Settings → fetch models).
    const settingsState = useSettingsStore.getState();
    const enabledProviders = settingsState.providers.filter((p) => p.enabled);

    for (const provider of enabledProviders) {
      if (provider.apiKey && provider.baseUrl && provider.models.length === 0) {
        await settingsState.fetchModels(provider.id);
      }
    }

    // Re-read after potential auto-fetch.
    const withPricing = buildModelsFromProviders(
      useSettingsStore.getState().providers,
    );

    if (withPricing.length > 0) {
      // Prefer: persisted model id → currently selected → first available.
      const persisted = loadPref(modelPrefKey);
      const persistedModel = persisted
        ? withPricing.find((m) => m.id === persisted)
        : undefined;
      const current = get().selectedModel;
      const stillExists = current && withPricing.some((m) => m.id === current.id);
      set({
        models: withPricing,
        selectedModel: persistedModel ?? (stillExists ? current : withPricing[0]),
      });
    } else {
      // No models configured — leave the picker empty; ModelSelector shows a
      // placeholder until models are added in Settings → Model Providers.
      set({ models: [], selectedModel: null });
    }
  },

  ensureSession: async () => {
    const existing = get().currentSessionId;
    if (existing) return existing;

    const model = get().selectedModel;
    if (!model) {
      logWarn("chatStore", "No model selected — cannot create session");
      return "";
    }
    try {
      // Code sessions carry the workspace path (permission rules, project
      // context); depwork sessions don't. The 4th arg is the backend
      // work_mode — the seam that makes one implementation serve both.
      const workspacePath =
        mode === "code" ? useAppStore.getState().workspacePath : undefined;
      // The picker list can lag behind Settings edits — resolve the context
      // window from the live provider config so a manual edit takes effect
      // on the next session immediately.
      const liveModel = useSettingsStore
        .getState()
        .providers.find((p) => p.id === model.providerId)
        ?.models.find((m) => m.id === model.id);
      const contextWindow = liveModel
        ? resolveContextWindow(liveModel.id, liveModel.contextWindow)
        : model.context_window;
      const session = await sessionApi.createSession(
        model.id,
        model.providerId,
        workspacePath ?? undefined,
        mode,
        contextWindow,
        // New sessions inherit the combo's current permission mode so the
        // per-session scope starts consistent with what the user sees.
        // read_only is transient (memory-only) — never persisted to the row.
        get().interactionMode === "read_only" ? undefined : get().interactionMode,
      );
      set({ currentSessionId: session.id, sessionTitle: session.title });
      return session.id;
    } catch {
      // Fallback: generate a local ID for browser dev mode
      const fallbackId = `${mode === "depwork" ? "depwork-" : "local-"}${Date.now()}`;
      set({ currentSessionId: fallbackId });
      return fallbackId;
    }
  },

  sendMessage: createSendMessage({ set, get, mode }),
  stopStreaming: async () => {
    // Stops the CURRENT session's turn only — other sessions' streams keep
    // running independently (multi-session concurrency).
    const sessionId = get().currentSessionId;
    if (!sessionId) return;
    await get().stopSessionStreaming(sessionId);
  },

  stopSessionStreaming: async (sessionId) => {
    const st = streamState(sessionId);
    if (!st.inFlight && !st.unlisten) return;    st.replayActive = false;
    st.gen += 1; // invalidate the in-flight turn's cleanup (stale guard)

    // Clean up the event listener immediately — before calling cancelOperation
    // so no late events from the backend can slip through.
    const unlisten = st.unlisten;
    st.unlisten = null;
    syncStreamingBus(sessionId, st);
    if (unlisten) {
      unlisten();
    }

    try {
      await systemApi.cancelOperation(sessionId);
    } catch {
      // Ignore — running outside Tauri or already stopped
    }
    st.paused = false;
    st.phase = "idle";
    const isCurrent = get().currentSessionId === sessionId;
    // Mark the stopped session's streaming messages as done in its OWN buffer
    // (a background session's messages aren't rendered here — the buffer
    // write is unconditional; the store sync only when it's the session shown).
    updateSessionMessages(
      st,
      sessionId,
      (msgs) => msgs.map((m) => (m.isStreaming ? { ...m, isStreaming: false } : m)),
      get,
      set,
      isCurrent
        ? { streamPhase: "idle", memoryRef: null, isStreaming: false, isPaused: false }
        : undefined,
    );
    // A queued message must not be lost when the turn is stopped — put it
    // back into the input instead of silently dropping it.
    const queued = st.queuedText;
    if (queued) {
      st.queuedText = null;
      if (isCurrent) {
        set({ queuedText: null, inputText: queued });
      }
    }
  },

  disposeSession: (sessionId) => {
    const st = streamStates.get(sessionId);
    if (!st) return;
    // Invalidate any in-flight turn and tear the listener down first so
    // late events can't write into the removed conversation.
    st.gen += 1;
    st.replayActive = false;
    st.queuedText = null;
    const unlisten = st.unlisten;
    st.unlisten = null;
    syncStreamingBus(sessionId, st);
    if (unlisten) unlisten();
    void systemApi.cancelOperation(sessionId).catch(() => {
      // Ignore — running outside Tauri or already stopped
    });
    streamStates.delete(sessionId);
    // The deleted session is the one in view — reset the live flags so the
    // input bar doesn't stay stuck on a dead session's stream.
    const s = get();
    if (s.currentSessionId === sessionId) {
      set({
        isStreaming: false,
        isPaused: false,
        streamPhase: "idle",
        queuedText: null,
        messages: [],
      });
    }
  },

  pauseStreaming: async () => {
    const sessionId = get().currentSessionId;
    if (!sessionId || !get().isStreaming || get().isPaused) return;
    try {
      await systemApi.pauseOperation(sessionId);
      streamState(sessionId).paused = true;
      set({ isPaused: true });
    } catch {
      // Ignore — running outside Tauri or already stopped
    }
  },

  resumeStreaming: async () => {
    const sessionId = get().currentSessionId;
    if (!sessionId || !get().isPaused) return;
    try {
      await systemApi.resumeOperation(sessionId);
      streamState(sessionId).paused = false;
      set({ isPaused: false });
    } catch {
      // Ignore — running outside Tauri or already stopped
    }
  },

  clearMessages: () => {
    // If a turn is mid-stream, dispose it first — otherwise its late
    // turn_end (with currentSessionId now null) keeps isStreaming true and
    // the input bar shows dead stop/pause controls until the next turn.
    const prev = get().currentSessionId;
    if (prev) get().disposeSession(prev);
    return set({
      messages: [],
      currentSessionId: null,
      sessionTitle: "New Session",
      streamPhase: "idle",
      compactions: [],
      memoryRef: null,
      isStreaming: false,
      isPaused: false,
      // Subagent records are keyed by per-run ids and otherwise never pruned —
      // clearing the conversation resets the accumulation.
      subagents: {},
      totalTokens: { prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 },
      contextChips: [],
      // A queued message belongs to the old session/turn — never auto-send it
      // into a fresh conversation.
      queuedText: null,
    });
  },
  dismissNotification: () => set({ notification: null }),

  // ── Recall (delete) a message and everything after it ──────
  deleteMessage: async (id) => {
    const state = get();
    const sid = state.currentSessionId;
    if (!sid) return;
    const st = streamState(sid);
    const idx = st.messages.findIndex((m) => m.id === id);
    if (idx < 0) return;

    // If the turn is still streaming, stop it first so no late events
    // can re-append blocks to a deleted message.
    if (state.isStreaming) {
      await get().stopStreaming();
    }

    const removed = st.messages.slice(idx);
    const userMsg = removed.find((m) => m.role === "user");
    const textContent = userMsg
      ? userMsg.blocks
          .filter((b) => b.type === "text")
          .map((b) => b.content)
          .join("\n")
      : "";

    const next = st.messages.slice(0, idx);
    st.messages = next;
    set({ messages: next, contextChips: [] });

    // Persist the truncation on the backend (best-effort).
    if (sid && textContent) {
      try {
        await sessionApi.deleteMessage(sid, textContent);
      } catch {
        // Ignore — UI is already updated; backend stays consistent next persist.
      }
    }
  },

  // ── Session restore (called by useSessionRestore) ────────
  isSessionStreaming: (id) => sessionStreaming(id),

  setSessionId: (id) =>
    set(() => {
      // Switching sessions does NOT stop the old session's stream (it keeps
      // running in the background). Only the queued-text view follows the
      // active session.
      const st = streamStates.get(id ?? "");
      const queuedText = st?.queuedText ?? null;
      if (queuedText && st && !sessionStreaming(id)) {
        // The turn that queued this message already ended while another
        // session was shown — restore it and auto-send now that the session
        // is back in view, so it never sits queued forever.
        st.queuedText = null;
        setTimeout(() => {
          if (get().currentSessionId === id) void get().sendMessage("queue");
        }, 0);
        return {
          currentSessionId: id,
          queuedText: null,
          inputText: queuedText,
          isStreaming: sessionStreaming(id),
          isPaused: st?.paused ?? false,
          streamPhase: st?.phase ?? "idle",
          // The subagents pane is per-session — a switch clears stale
          // records from the previous session.
          subagents: {},
          // Project this session's per-session buffer into the store view.
          messages: streamStates.get(id ?? "")?.messages ?? [],
        };
      }
      return {
        currentSessionId: id,
        queuedText,
        isStreaming: sessionStreaming(id),
        isPaused: st?.paused ?? false,
        streamPhase: st?.phase ?? "idle",
        subagents: {},
        // Project this session's per-session buffer into the store view.
        messages: streamStates.get(id ?? "")?.messages ?? [],
      };
    }),
  setSessionTitle: (title) => set({ sessionTitle: title }),
  setSessionLoading: (loading) => set({ sessionLoading: loading }),
  setMessages: (messages) =>
    set((s) => {
      // Merge the restored history with this session's in-flight buffer: a
      // turn the backend has NOT yet persisted (mid-stream) would otherwise be
      // wiped by the replacement. History wins for what it has; in-flight
      // messages absent from history are appended in order.
      const sid = s.currentSessionId;
      let merged = messages;
      if (sid) {
        const st = streamState(sid);
        const historyIds = new Set(messages.map((m) => m.id));
        const inflight = st.messages.filter((m) => !historyIds.has(m.id));
        merged = inflight.length > 0 ? [...messages, ...inflight] : messages;
        st.messages = merged;
      }
      return {
        messages: merged,
        // A restored session starts fresh on usage accounting — the store's
        // totals cover the live session only, not its full history.
        totalTokens: { prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 },
      };
    }),

  // ── Context chip actions ───────────────────────────────────
  // Dedup by type + path: a double-click (or a stale-closure handler) must
  // not attach the same file/URL twice. Separators are normalized so a
  // `C:\a` native picker path and a `C:/a` tree path are the same file.
  addContextChip: (chip) =>
    set((s) => {
      const key = (c: ContextChip) =>
        `${c.type}:${(c.path ?? "").replace(/\\/g, "/").toLowerCase()}`;
      const dup = s.contextChips.some((c) => key(c) === key(chip));
      return dup ? s : { contextChips: [...s.contextChips, chip] };
    }),

  removeContextChip: (id) =>
    set((s) => ({ contextChips: s.contextChips.filter((c) => c.id !== id) })),

  clearContextChips: () => set({ contextChips: [] }),

  addFileContext: async () => {
    const paths = await pickFiles();
    const prefix = mode === "depwork" ? "dw-file-" : "file-";
    for (const p of paths) {
      get().addContextChip({
        id: `${prefix}${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        type: "file",
        name: p.split(/[\\/]/).pop() ?? p,
        path: p,
      });
    }
  },

  addFolderContext: async () => {
    const paths = await pickFolders();
    const prefix = mode === "depwork" ? "dw-folder-" : "folder-";
    for (const p of paths) {
      get().addContextChip({
        id: `${prefix}${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        type: "folder",
        name: p.split(/[\\/]/).pop() ?? p,
        path: p,
      });
    }
  },

  addUrlContext: (url) => {
    get().addContextChip({
      id: `url-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      type: "url",
      name: url,
      path: url,
    });
  },

  // ── Paper chip (depwork only — a document the agent opened/generated).
  // Code-mode instances carry the action too (API uniformity), but no code
  // UI calls it.
  addPaperContext: (title: string) => {
    get().addContextChip({
      id: `paper-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      type: "paper",
      name: title,
      path: title,
    });
  },

  // ── Interaction mode actions ────────────────────────────────
  setInteractionMode: (mode) => {
    set({
      interactionMode: mode,
      manualModeOverride: true,
    });
    savePref(modePrefKey, mode);
  },

  // Internal: auto-detection sets mode without claiming manual override,
  // and does NOT persist (auto-detection must not overwrite the user's choice).
  _autoSetMode: (mode) => set({ interactionMode: mode }),

  // ── Reasoning mode action ──────────────────────────────────
  setReasoningMode: (mode) => {
    set({ reasoningMode: mode });
    savePref(PREF_REASONING, mode);
  },

  // ── Ask user actions ───────────────────────────────────────
  setPendingAskUser: (req) => set({ pendingAskUser: req }),

  respondAskUser: async (response) => {
    const req = get().pendingAskUser;
    if (!req) return;
    await askUserApi.respond(req.request_id, response);
    set({ pendingAskUser: null });
  },

  // ── MCP elicitation actions ────────────────────────────────
  setPendingElicitation: (req) => set({ pendingElicitation: req }),

  respondElicitation: async (elicitationId, action, content) => {
    await elicitationApi.respond(elicitationId, action, content);
    set({ pendingElicitation: null });
  },
  }));

  // Keep the picker / input-bar model list in sync with Settings edits:
  // when provider config changes (context window, model list, renames), the
  // display list is rebuilt from the live settings — no auto-fetch here,
  // that only happens on explicit loadModels. The selected model keeps its
  // identity (id + providerId) but picks up fresh metadata like the window.
  useSettingsStore.subscribe((state, prev) => {
    if (state.providers === prev.providers) return;
    const withPricing = buildModelsFromProviders(state.providers);
    store.setState((s) => {
      if (withPricing.length === 0) return { models: [], selectedModel: null };
      const current = s.selectedModel;
      const keep =
        current &&
        withPricing.find(
          (m) => m.id === current.id && m.providerId === current.providerId,
        );
      return { models: withPricing, selectedModel: keep ?? withPricing[0] };
    });
  });

  return store;
}

/** Code-mode chat store (standard / plan_execute / coordinator…). */
export const useChatStore = createChatStore("code");
