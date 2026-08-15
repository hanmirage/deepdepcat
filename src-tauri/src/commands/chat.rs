//! Chat commands — send messages, stream responses, manage chat.

use crate::agent::agent_builder::AgentBuilder;
use crate::agent::running::{RunningTurnInfo, RunningTurnStatus};
use crate::commands::chat_types::SendChatResult;
use crate::bootstrap::AppState;
use crate::core::stream::emit_stream;
use crate::core::types::{ContextChip, TurnSnapshot};
use std::sync::Arc;
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Pull the authoritative terminal snapshot of a turn (gap recovery). The
/// snapshot exists once the turn has ended — mid-turn requests return
/// `None` (live deltas are the source of truth while the turn runs).
#[tauri::command]
pub fn get_turn_snapshot(session_id: String, turn_id: String) -> Option<TurnSnapshot> {
    crate::core::stream::get_turn_snapshot(&session_id, &turn_id)
}

/// Send a chat message and stream the response.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn send_chat_message(
    session_id: String,
    message: String,
    mode: Option<String>,
    work_mode: Option<String>,
    context_chips: Option<Vec<ContextChip>>,
    reasoning_mode: Option<String>,
    agent_name: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SendChatResult, String> {
    info!(session_id = %session_id, message_len = message.len(), "Sending chat message");

    // Create cancellation token. Registered AFTER chat_state is checked out:
    // a queued sender must NOT overwrite the running loop's token, or a
    // user cancel would target the wrong (already-returned) invocation.
    let cancel_token = CancellationToken::new();

    // Read config and build all one-shot resources before touching the session lock.
    let workspace = state.workspace.read().map_err(|e| e.to_string())?.clone();

    let usage_tracker = state.usage_tracker(&session_id).await;

    let agent_mode = crate::commands::chat_types::parse_agent_mode(mode.as_deref());
    // Product work mode — Code (default) or Depwork. Filters the tool
    // registry and selects the mode-specific system prompt.
    let work_mode = crate::toolkit::WorkMode::parse(work_mode.as_deref());

    // Optional custom agent persona for THIS session: its body overlays the
    // mode prompt and its permissions + tool allowlist are enforced (M9
    // semantics). Unknown / wrong-mode / "default" names resolve to None.
    let custom_agent = agent_name.as_deref().and_then(|name| {
        crate::agent::definition::resolve_for_main(workspace.as_deref(), work_mode, name)
    });

    // Create file state tracker for checkpoint/rewind. If the session already
    // has one (a previous turn this process), reuse it — otherwise create a
    // fresh tracker and restore its persisted rewind points from the database
    // so checkpoints survive restarts.
    let file_state_tracker = {
        let mut trackers = state.file_state_trackers.lock().await;
        if let Some(existing) = trackers.get(&session_id).cloned() {
            existing
        } else {
            let tracker = crate::workspace::checkpoint::FileStateTracker::new(workspace.clone());
            if let Err(e) = tracker.load_from_db(&session_id, &state.db).await {
                tracing::warn!(session_id, error = %e, "Failed to restore rewind points");
            }
            trackers.insert(session_id.clone(), tracker.clone());
            tracker
        }
    };

    // Image chips are split by main model capability: vision-capable models
    // receive the picture natively (injected as an initial image part, see
    // below); text-only models (DeepSeek) get an automatic transcription from
    // the configured vision model. A pasted/picked image arrives as a data
    // URL, a dragged file as a path the backend reads directly. Image chips
    // are removed from the context either way — the model never resolves
    // their paths itself.
    let (chips, image_inputs, image_notes) =
        crate::commands::chat_chips::split_image_chips(context_chips, &state, &session_id).await;

    // The session's provider hint rides into tool contexts so meta-tools
    // (the `agent` decompose call) route their internal LLM calls to the
    // SAME provider as the session — a custom-provider model must never fall
    // back to the first enabled provider (the #102 model-routing bug class).
    let session_provider = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .get_session(&session_id)
            .ok()
            .map(|s| s.provider.clone())
    };

    let built_agent = AgentBuilder::from_state(&state, workspace)?
        .with_mode(agent_mode)
        .with_work_mode(work_mode)
        .with_custom_agent(custom_agent)
        .with_context_chips(chips)
        .with_usage_tracker(usage_tracker.clone())
        .with_debug_mode(state.debug_mode())
        .with_file_state_tracker(file_state_tracker.clone())
        .with_reasoning_effort(reasoning_mode)
        .with_provider(session_provider)
        .with_interjections(Arc::new(Mutex::new(
            crate::agent::interjection::InterjectionRegistry::new(),
        )))
        .build();

    let agent_loop = built_agent.loop_;

    // Session-level concurrency cap: bound how many DISTINCT sessions run an
    // agent loop at once. Acquired BEFORE take_chat_state so a session that
    // waits (beyond the cap) or is cancelled while waiting never holds a
    // checked-out ChatState — no state is left stranded. The wait is
    // cancellable (a user interrupt aborts the wait cleanly).
    //
    // Same-session queued replays skip this: they never reach this point
    // (the busy path below returns "queued" without owning a permit), and a
    // running loop drains its own queue without re-entering send_message.
    let _session_permit = {
        let acquire = state.session_concurrency.clone().acquire_owned();
        tokio::select! {
            permit = acquire => {
                Some(permit.map_err(|e| format!("Session concurrency semaphore closed: {e}"))?)
            }
            _ = cancel_token.cancelled() => {
                // Wait aborted by user cancel — nothing was taken, nothing to
                // restore; return cleanly.
                return Ok(SendChatResult::cancelled());
            }
        }
    };

    // ── Take the ChatState OUT of the session manager, then release the lock. ──
    // This is critical: the lock must not be held across the LLM streaming call,
    // or any concurrent session operation (including the frontend fetching
    // session metadata) would deadlock.
    //
    // If the state is already checked out, another agent loop is running for
    // this session — queue the prompt and return immediately. The running
    // loop drains the queue after its current turn completes. In that case
    // the concurrency permit is released here (this invocation never runs).
    let mut chat_state = {
        let mut sessions = state.sessions.lock().await;
        match sessions.take_chat_state(&session_id) {
            Ok(cs) => cs,
            Err(_) => {
                drop(_session_permit);
                // Image attachments cannot be replayed through the prompt
                // queue: the vision pipeline (transcription / image parts)
                // only runs on the direct path. Rejecting with an explicit
                // error beats silently dropping the user's pictures — the
                // text-only message can be resent once the turn finishes.
                if !image_inputs.is_empty() {
                    return Err("Agent is busy and image attachments cannot be queued — \
                         retry once the current turn finishes"
                        .to_string());
                }
                let prompt_id = crate::core::ids::generate_id();
                {
                    let mut queues = state.prompt_queues.lock().await;
                    let queue = queues.entry(session_id.clone()).or_default();
                    queue
                        .push(prompt_id.clone(), message.clone())
                        .map_err(|e| format!("Agent busy: {e}"))?;
                }
                info!(
                    session_id = %session_id,
                    prompt_id = %prompt_id,
                    "Session busy — prompt queued for replay"
                );
                return Ok(SendChatResult::queued(prompt_id));
            }
        }
    };
    // Full-run trace id — one identifier propagated through every stream
    // event this turn emits (chat-stream → SSE → ACP), so a single task is
    // traceable across protocols and log lines.
    let trace_id = crate::core::ids::trace_id();
    chat_state.trace_id = Some(trace_id.clone());
    info!(session_id = %session_id, trace_id = %trace_id, "Turn trace started");

    // Apply the custom agent's body as the session persona overlay — the
    // mode's base prompt + boundary stay intact underneath (Extend
    // semantics). Empty bodies keep the standard persona.
    if let Some(prompt) = built_agent.system_prompt.as_deref() {
        if chat_state.system_prompt.trim().is_empty() {
            chat_state.system_prompt = prompt.to_string();
        } else {
            chat_state.system_prompt = format!("{}\n\n{}", chat_state.system_prompt.trim(), prompt);
        }
    }

    // This invocation owns the session now — register the cancel token so
    // cancel_operation can interrupt the (possibly queued-backlog) run.
    state
        .register_cancellation(&session_id, cancel_token.clone())
        .await;
    // Register the pause channel so pause_operation/resume_operation can
    // suspend the loop at its checkpoints.
    state.register_pause(&session_id).await;

    // ── Image split by main model capability ──────────────────────────
    // Vision-capable models receive the compressed pictures natively (as an
    // initial image part consumed by the first API request); text-only models
    // (DeepSeek) get the automatic vision-model transcription injected as
    // text — the model never sees image bytes and never resolves paths.
    let mut message = message;
    if let Err(e) = crate::commands::chat_image::prepare_images(
        &state,
        &session_id,
        &mut chat_state,
        &usage_tracker,
        image_inputs,
        image_notes,
        &mut message,
    )
    .await
    {
        // ── Cleanup on transcription failure ─────────────
        // This invocation OWNS the session (checked out above): a failure
        // here must put the ChatState BACK and clear the cancel/pause
        // registrations, or the session stays permanently checked out —
        // every later send hits the "busy" queue path and is never consumed
        // (#88 audit H1: a leaked checkout bricks the session).
        {
            let mut sessions = state.sessions.lock().await;
            let _ = sessions.put_chat_state(&session_id, chat_state);
            let _ = sessions.persist_session(&session_id);
            let _ = sessions.persist_messages(&session_id);
        }
        state.remove_cancellation(&session_id).await;
        state.remove_pause(&session_id).await;
        emit_stream(
            &app,
            crate::core::types::StreamEvent::Error {
                turn_id: String::new(),
                session_id: session_id.clone(),
                message: e.clone(),
                trace_id: None,
            },
        );
        return Err(e);
    }

    // ── Persistence: expose this turn as a background session ─────────
    // The turn now runs independently of any frontend connection — the
    // user may switch sessions while it finishes. Register it so the
    // activity panel can list it, and broadcast start/completion events.
    let running_info = RunningTurnInfo {
        session_id: session_id.clone(),
        turn_id: trace_id.clone(),
        started_at_ms: SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        message_preview: crate::agent::running::turn_message_preview(&message),
        work_mode: work_mode.as_str().to_string(),
        status: RunningTurnStatus::Running,
    };
    state.running_turns.register(running_info.clone()).await;
    let _ = app.emit("agent-turn-started", &running_info);

    // Run the first message, then replay any queued prompts in order. The
    // same agent loop and cancellation token drive every queued message, so
    // a user interrupt aborts the whole backlog. The loop value is the last
    // successfully processed turn ID (same contract as before the queue).
    let mut current_message = message;
    let last_msg_id = loop {
        // Run the agent loop with full ownership of chat_state — no lock held.
        let result = agent_loop
            .run(
                &app,
                &session_id,
                &mut chat_state,
                &current_message,
                &cancel_token,
                state.debug_mode(),
                Some(file_state_tracker.clone()),
                Some(&state.skill_engine),
            )
            .await;

        // ── Self-evolution: background learning extraction ──────────
        // After a turn that actually changed files, extract non-obvious
        // learnings in the background (memory + workspace learnings.md),
        // throttled to once per 10 minutes per session so the LLM cost
        // stays bounded. Failures are silent — never block the turn.
        crate::commands::chat_capture::maybe_capture_learnings(
            &state,
            &session_id,
            &chat_state,
            result.is_ok(),
        )
        .await;

        // ── Self-evolution: background procedure capture ────────────
        // After the same successful turn, extract 0-1 reusable workflows
        // into the project procedures.md (mode-locked to this session),
        // throttled like learning: once per 10 minutes per session.
        // Failures are silent — never block the turn.
        crate::commands::chat_capture::maybe_capture_procedure(
            &state,
            &session_id,
            &chat_state,
            result.is_ok(),
            work_mode,
        )
        .await;

        // ── Self-evolution: project-cognition generation ────────────
        // After a successful turn, generate the workspace's project map
        // (`.deepdepcat/project-cognition.md`) once — the LLM architecture
        // note over the deterministic module snapshot. Background, silent.
        crate::commands::chat_capture::maybe_capture_project_cognition(
            &state,
            &chat_state,
            result.is_ok(),
        )
        .await;

        // ── Put the ChatState back and persist. ──
        {
            let mut sessions = state.sessions.lock().await;
            let _ = sessions.put_chat_state(&session_id, chat_state);
            let _ = sessions.persist_session(&session_id);
            let _ = sessions.persist_messages(&session_id);
        }

        let msg_id = match result {
            Ok(id) => id,
            Err(e) => {
                // The run ended abnormally (cancel / tool error / loop exit):
                // finalize the turn + restore the session's permission state
                // so a cancelled plan phase never leaves the session read-only.
                state
                    .finalize_run(
                        &app,
                        &session_id,
                        if e.is_cancelled() { "cancelled" } else { "error" },
                    )
                    .await;
                // Frontends waiting on a backend-queued replay (kept their
                // listener alive after "queued:...") must not hang forever
                // when this turn fails before the backlog is drained —
                // notify them so the wait ends.
                emit_stream(
                    &app,
                    crate::core::types::StreamEvent::Error {
                        turn_id: String::new(),
                        session_id: session_id.clone(),
                        message: e.to_string(),
                        trace_id: None,
                    },
                );
                return if e.is_cancelled() {
                    // A user interrupt aborts the WHOLE backlog — not just the
                    // in-flight message. Queued prompts (pushed while the
                    // session was busy) would otherwise replay on the next
                    // send, silently running a prompt the user already
                    // cancelled (possibly editing files, long after the
                    // interrupt). Drop them here.
                    {
                        let mut queues = state.prompt_queues.lock().await;
                        queues.remove(&session_id);
                    }
                    Ok(SendChatResult::cancelled())
                } else {
                    Err(e.to_string())
                };
            }
        };

        // Drain the next queued prompt (if any).
        let next = {
            let mut queues = state.prompt_queues.lock().await;
            queues
                .get_mut(&session_id)
                .and_then(|q| q.pop())
                .map(|entry| entry.text)
        };
        let Some(next_message) = next else {
            break msg_id;
        };
        info!(session_id = %session_id, "Replaying queued prompt");
        let rechecked_out = {
            let mut sessions = state.sessions.lock().await;
            sessions.take_chat_state(&session_id)
        };
        chat_state = match rechecked_out {
            Ok(cs) => cs,
            Err(e) => {
                // The session disappeared while the backlog was draining —
                // clean up every registration exactly like the run-error
                // path above (a leftover cancel/pause registration or a
                // parked plan-mode state would corrupt the next turn).
                state.finalize_run(&app, &session_id, "error").await;
                return Err(e.to_string());
            }
        };
        current_message = next_message;
    };

    // Finalize the run + restore the session's permission state on the normal
    // end — a session whose run finished while still in a plan phase (model
    // stopped without exit_plan_mode) must not stay read-only for its next
    // turn.
    state.finalize_run(&app, &session_id, "completed").await;
    Ok(SendChatResult::accepted(last_msg_id))
}
