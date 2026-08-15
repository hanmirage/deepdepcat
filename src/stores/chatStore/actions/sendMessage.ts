/**
 * sendMessage action — the streaming turn engine, extracted from
 * createChatStore so the store factory stays reviewable.
 *
 * Owns: busy handling, turn generation, the chat-stream listener with
 * RAF-batched flushes, invoke, queued-replay, and the stale-turn guards.
 */

import { chatApi, connectChatStream, permissionApi } from "@/lib/tauri";
import { logWarn } from "@/lib/logger";
import { buildStreamListener } from "../streamTurn/listener";
import { useAppStore } from "@/stores/appStore";
import { useDepworkStore } from "@/stores/depworkStore";
import { autoTitleSession } from "@/lib/sessionTitle";
import { useSettingsStore } from "@/stores/settingsStore";
import { detectMode, isConfirmationReply } from "../modeDetect";
import i18n from "@/i18n";
import { streamState, sessionStreaming, syncStreamingBus } from "../streamState";
import { updateSessionMessages } from "../sessionMessages";
import type { ChatState, ChatWorkMode } from "../types";
import type { AgentMode, UIMessage } from "@/types";

export interface SendMessageContext {
  set: (partial: Partial<ChatState> | ((state: ChatState) => Partial<ChatState>)) => void;
  get: () => ChatState;
  mode: ChatWorkMode;
}

export function createSendMessage({ set, get, mode }: SendMessageContext) {
  return async (whenBusy: "queue" | "interrupt" = "queue"): Promise<void> => {
    const text = get().inputText.trim();
    if (!text) return;

    // ── Busy handling: queue or interrupt ────────────────────
    // Busy is PER-SESSION: another session streaming never blocks this one.
    // Only the SAME session's in-flight turn queues (or interrupts).
    const busySessionId = get().currentSessionId;
    if (sessionStreaming(busySessionId)) {
      if (whenBusy === "interrupt") {
        await get().stopStreaming();
      } else {
        // Queue the text; it will be sent when this session's turn ends.
        // Pin the work mode NOW (the mode the message was queued under) —
        // the user may switch surfaces before the turn ends, and the
        // auto-send must execute under the mode it was queued in.
        const st = streamState(busySessionId ?? "");
        st.queuedText = text;
        st.queuedWorkMode = useAppStore.getState().mode;
        set({ queuedText: text, inputText: "" });
        return;
      }
    }

    // ── Rule 2: Read-only confirmation shortcut ─────────────
    // If the user is in read-only mode and types a bare confirmation word,
    // switch to accept-edits WITHOUT sending a new message. Code-only.
    if (
      mode === "code" &&
      get().interactionMode === "read_only" &&
      isConfirmationReply(text)
    ) {
      get()._autoSetMode("accept_edits");
      // Make the confirmation REAL on the backend too — the session may
      // still hold a read-only override, and the next message must not be
      // silently locked again.
      const sid = get().currentSessionId;
      if (sid) void permissionApi.setMode("accept_edits", sid).catch(() => {});
      set({ inputText: "" });
      return;
    }

    // ── Rule 1: Auto-detect mode from message keywords ─────
    // Only when the user hasn't manually selected a mode.
    if (!get().manualModeOverride) {
      const detected = detectMode(mode, text);
      if (detected !== get().interactionMode) {
        get()._autoSetMode(detected);
      }
    }

    const model = get().selectedModel;
    if (!model) {
      logWarn("chatStore", "No model selected — cannot send message");
      return;
    }

    // ── Double-send guard ────────────────────────────────────
    // isStreaming isn't set until after ensureSession resolves, so two rapid
    // sends could both pass the busy check above. The lock closes that window
    // on the pre-ensure session; released once the stream flag is set.
    const preSt = streamState(busySessionId ?? "");
    if (preSt.inFlight) return;
    preSt.inFlight = true;

    // Ensure we have a valid session before sending
    const sessionId = await get().ensureSession();
    if (!sessionId) {
      preSt.inFlight = false;
      return;
    }

    // ── Real permission-mode parity ─────────────────────────────────────
    // The permission layer must enforce exactly what the input bar shows.
    // Auto-detected modes and user choices both land on the session row;
    // `read_only` is deliberately excluded — it is a transient read-only
    // posture, and writing it into the session row was the "一直卡在计划模式"
    // bug class. The frontend id IS the backend wire string now.
    const effectiveMode = get().interactionMode;
    if (sessionId && effectiveMode !== "read_only") {
      void permissionApi.setMode(effectiveMode, sessionId).catch(() => {
        // Best-effort: a failure here only affects display parity; the
        // backend keeps its own authoritative mode.
      });
    }

    // The session may have been created by ensureSession — bind the stream
    // state to the FINAL session (both are usually the same).
    const st = streamState(sessionId);
    if (st !== preSt) {
      preSt.inFlight = false;
      st.inFlight = true;
    }
    syncStreamingBus(sessionId, st);
    // ── Turn generation ──────────────────────────────────────
    // Bumped per session. A stale turn (interrupted by stopStreaming then
    // followed by a new send on the SAME session) sees its generation change
    // and skips cleanup so it can't clobber the new turn's state.
    const gen = ++st.gen;

    // Auto-title the session from the first user message. Only fires when the
    // title is still the backend default ("New Session"); best-effort, never
    // blocks sending. The .then re-checks the current title before writing to
    // avoid a stale write clobbering a newer title (interrupt-then-resend race).
    const sessionTitle = get().sessionTitle;
    void autoTitleSession(sessionId, sessionTitle, text).then((t) => {
      if (t && get().sessionTitle === sessionTitle) set({ sessionTitle: t });
    });

    // Per-mode message id prefix — keeps the two surfaces' message ids
    // distinguishable (id space provenance for session restore).
    const midPrefix = mode === "depwork" ? "dw-" : "";
    const assistantId = `${midPrefix}a-${Date.now()}`;

    const currentChips = get().contextChips;

    const userMsg: UIMessage = {
      id: `${midPrefix}u-${Date.now()}`,
      role: "user",
      blocks: [{ type: "text", content: text }],
      model: model.id,
      timestamp: Date.now(),
      // Code renders the attached chips on the user message. Depwork keeps
      // the bubble clean: the chips are passed to the API only (mapped to
      // `file`), never shown on the message.
      contextChips:
        mode === "code" && currentChips.length > 0 ? currentChips : undefined,
    };

    const assistantMsg: UIMessage = {
      id: assistantId,
      role: "assistant",
      blocks: [],
      model: model.id,
      timestamp: Date.now(),
      isStreaming: true,
    };

    updateSessionMessages(
      st,
      sessionId,
      (msgs) => [...msgs, userMsg, assistantMsg],
      get,
      set,
      { inputText: "", isStreaming: true, isPaused: false },
    );
    st.inFlight = false;

    // Product work mode — mirrors the active app surface (code / depwork).
    // The backend filters the tool registry and picks the system prompt.
    // An auto-send uses the mode pinned when the message was queued so a
    // mid-turn surface switch cannot leak a message into the wrong mode.
    const workMode = st.queuedWorkMode ?? useAppStore.getState().mode;
    st.queuedWorkMode = null;
    const expectedSessionId = sessionId;
    const finalizedRef = { current: false };
    const unlistenRef = { current: null as (() => void) | null };
    const turn = buildStreamListener({
      get,
      set,
      st,
      assistantId,
      expectedSessionId,
      gen,
      mode,
      finalizedRef,
      unlistenRef,
    });
    const unlisten = await connectChatStream(turn.handler, {
      onReconnect: () => turn.onTransportReconnect(),
    });
    st.unlisten = unlisten;
    unlistenRef.current = unlisten;

    // Call the backend command — pass sessionId + mode + contextChips
    // The invoke Promise resolves when the backend finishes streaming.
    // The `done` or `error` event may arrive just before or after resolve —
    // either way, we unlisten after resolve to guarantee no late events leak.

    // Map frontend interaction/execution modes to backend AgentMode.
    // code: accept_edits/full_access → undefined (backend Standard mode),
    // read_only → "plan_execute", explicit execution modes pass through.
    // depwork: the permission layer governs execution (只读 blocks tools,
    // 完全放行 allows, 接受编辑 prompts) — no loop-level mode needed.
    const interactionMode = get().interactionMode;
    const executionMode = get().agentMode;
    const agentMode: AgentMode | undefined =
      mode === "code"
        ? interactionMode === "read_only"
          ? "plan_execute"
          : executionMode !== "standard"
            ? executionMode
            : undefined
        : undefined;

    const reasoningMode = get().reasoningMode;
    // "auto" tiers effort per intent when DeepSeek optimization is on (light
    // turns cheaper, heavy work keeps max); off → fixed "high". Explicit
    // low/high/max always win. Code mode uses the input bar; depwork has no
    // selector, so the setting is authoritative there.
    const effectiveReasoning =
      mode === "code"
        ? reasoningMode === "auto"
          ? useSettingsStore.getState().general.deepseekAutoReasoning ? "auto" : "high"
          : reasoningMode
        : useSettingsStore.getState().general.deepseekAutoReasoning
          ? "auto"
          : "high";
    const contextChips = get().contextChips;
    // Depwork "paper" chips map to `file` for the API — code never
    // produces paper chips, so the mapping is a no-op there.
    let apiContextChips =
      contextChips.length > 0
        ? contextChips.map((c) => ({
            ...c,
            type: c.type === "paper" ? "file" : c.type,
          }))
        : undefined;
    // Depwork's document directory reaches the agent WITHOUT living in the
    // input-box chips (the folder picker button already shows it — a visible
    // chip there was a duplicate). Inject it per-turn as a folder chip so
    // the backend still sees the attached directory.
    if (mode === "depwork") {
      const rootPath = useDepworkStore.getState().rootPath;
      if (rootPath && !apiContextChips?.some((c) => c.type === "folder" && c.path === rootPath)) {
        const name = rootPath.split(/[\\/]/).pop() ?? rootPath;
        apiContextChips
          ? apiContextChips.push({ id: `dw-folder-${Date.now()}`, type: "folder", name, path: rootPath })
          : (apiContextChips = [{ id: `dw-folder-${Date.now()}`, type: "folder", name, path: rootPath }]);
      }
    }
    // Custom agent persona ("" = default). Sent with every message so a
    // mid-turn persona switch takes effect on the next turn.
    const selectedAgent = get().selectedAgent;

    try {
      const sendResult = await chatApi.sendMessage(
        sessionId,
        text,
        agentMode,
        workMode,
        apiContextChips,
        effectiveReasoning,
        selectedAgent || undefined,
      );
      // Backend queues prompts sent while the agent is busy — surface it and
      // KEEP the listener + streaming state alive: the running turn will
      // replay the prompt when it finishes (same invoke, fresh turn_id), and
      // its turn_start/turn_end events must reach this listener. Structured
      // envelope since #79 (was the "queued:..." magic string).
      if (sendResult.kind === "queued") {
        if (st.replayActive) {
          // Another queued send already holds the replay listener — drop
          // ours so the replayed turn is rendered exactly once.
          unlisten();
          st.unlisten = null;
        } else {
          st.replayActive = true;
        }
        finalizedRef.current = true; // skip finally cleanup — stay alive for replay
        set({
          notification: i18n.t("notifications.queuedWaiting", {
            defaultValue: "已排队：上一个任务完成后将自动处理该消息。",
          }),
        });
        return;
      }
    } catch (e) {
      st.inFlight = false;
      // A stale turn's failure must not kill the newer turn's listener.
      if (gen !== st.gen) return;
      // If the backend's error event already finalized this turn (it emits
      // one when a turn fails before draining queued replays), the listener
      // is gone and the error text was already appended — skip.
      if (st.unlisten === null) return;
      st.replayActive = false;
      turn.flushProgress();
      st.phase = "idle";
      const s = get();
      updateSessionMessages(
        st,
        sessionId,
        (msgs) =>
          msgs.map((m) =>
            m.id === assistantId
              ? { ...m, blocks: [...m.blocks, { type: "text" as const, content: `Failed to send: ${e}` }], isStreaming: false }
              : m,
          ),
        get,
        set,
        s.currentSessionId === sessionId
          ? { streamPhase: "idle", isStreaming: false, isPaused: false }
          : undefined,
      );
      unlisten();
      st.unlisten = null;
      syncStreamingBus(sessionId, st);
      finalizedRef.current = true;
    } finally {
      st.inFlight = false;
      // The invoke resolved, but the SSE channel and the invoke result are
      // separate transports with no ordering guarantee — the terminal
      // turn_end/error may still be in flight. Tearing the listener down
      // HERE would drop it: the message finalizes without its change summary
      // and turn outcome (the ✓完成 badge). Keep the listener armed so the
      // terminal event finalizes normally (it unlistens itself); a bounded
      // fallback prevents a leak if the event never arrives.
      if (!finalizedRef.current && gen === st.gen) {
        setTimeout(() => {
          if (finalizedRef.current || gen !== st.gen) return;
          turn.flushProgress();
          turn.flushPending();
          st.phase = "idle";
          unlisten();
          st.unlisten = null;
          syncStreamingBus(sessionId, st);
          const s = get();
          updateSessionMessages(
            st,
            sessionId,
            (msgs) =>
              msgs.map((m) =>
                m.id === assistantId ? { ...m, isStreaming: false } : m,
              ),
            get,
            set,
            s.currentSessionId === sessionId
              ? { streamPhase: "idle", contextChips: [], isStreaming: false }
              : undefined,
          );
          // A queued message must not sit forever if the terminal event never
          // arrives — restore it to the input instead of dropping it.
          const queued = st.queuedText;
          if (queued && get().currentSessionId === sessionId) {
            st.queuedText = null;
            set({ queuedText: null, inputText: queued });
          }
        }, 2000);
      }
    }
  };
}
