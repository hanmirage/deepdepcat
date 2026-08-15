//! Chat image pipeline — attached pictures are split by main model
//! capability: vision-capable models receive compressed image parts natively
//! (consumed by the first API request); text-only models (DeepSeek) get the
//! automatic vision-model transcription injected as text — the model never
//! sees image bytes and never resolves paths.

use crate::agent::chat_state::ChatState;
use crate::agent::image_transcribe::ImageInput;
use crate::bootstrap::AppState;
use crate::observability::usage::SessionUsageTracker;
use base64::Engine as _;

/// Prepare the user message for attached images.
///
/// On transcription failure the session is fully restored (chat state put
/// back, cancel/pause registrations cleared, error event emitted) and an
/// error is returned for the caller to propagate.
pub async fn prepare_images(
    state: &AppState,
    session_id: &str,
    chat_state: &mut ChatState,
    usage_tracker: &SessionUsageTracker,
    image_inputs: Vec<ImageInput>,
    image_notes: Vec<(String, String)>,
    message: &mut String,
) -> Result<(), String> {
    if image_inputs.is_empty() {
        return Ok(());
    }

    let can_see_images = {
        let sessions = state.sessions.lock().await;
        sessions.model_catalog().supports_vision(&chat_state.model)
    };
    if can_see_images {
        let mut parts: Vec<crate::core::types::ContentPart> = Vec::new();
        for img in image_inputs {
            match crate::core::image_codec::compress_image_for_conversation(img.bytes, img.mime)
            {
                Ok((encoded, out_mime)) => parts.push(crate::core::types::ContentPart::Image {
                    source_type: "base64".to_string(),
                    media_type: out_mime,
                    data: base64::engine::general_purpose::STANDARD.encode(encoded),
                }),
                Err(e) => {
                    tracing::warn!(error = %e, "Image compression failed for multimodal main model");
                }
            }
        }
        if !parts.is_empty() {
            chat_state.set_initial_image_parts(parts);
        }
    } else {
        let transcribed = crate::agent::image_transcribe::transcribe_images(
            state,
            session_id,
            image_inputs,
        )
        .await;
        let (descriptions, transcribe_usage) = match transcribed {
            Ok((d, usage)) => (d, usage),
            Err(e) => {
                // The caller OWNS the session checkout — it is responsible
                // for restoring ChatState + clearing registrations on the
                // error path (see chat.rs).
                return Err(e.to_string());
            }
        };
        // Vision transcription is a real LLM call — record its usage into
        // the session total (which seeds the turn budget and persists to
        // the usage pages) and the per-session tracker. Previously the
        // vision tokens were invisible to every accounting surface
        // (audit H7 residual).
        if transcribe_usage.total() > 0 {
            chat_state.total_usage.add(&transcribe_usage);
            usage_tracker.record_llm_usage(0, &transcribe_usage);
        }
        let blocks: Vec<String> = descriptions
            .iter()
            .map(|d| crate::agent::image_transcribe::render_image_description_block(d))
            .collect();
        *message = format!("{}\n\n{message}", blocks.join("\n\n"));
        // Text-only main model path: keep the resolvable image paths on
        // the session state so subagents spawned later in this turn can
        // `visual_describe` the attached pictures by path. Multimodal
        // parents never set this — their pictures travel as image parts.
        chat_state.attached_image_notes = image_notes.clone();
    }

    // Attach the image name + resolvable path list so the model knows it can
    // `visual_describe` any attached picture whose automatic transcription
    // is not detailed enough (exact text, fine print, specific UI elements).
    if !image_notes.is_empty() {
        let notes: Vec<String> = image_notes
            .iter()
            .map(|(name, path)| format!("- {name} — 路径: {path}"))
            .collect();
        *message = format!("{}\n\n## Attached Images\n{}", *message, notes.join("\n"));
    }
    Ok(())
}
