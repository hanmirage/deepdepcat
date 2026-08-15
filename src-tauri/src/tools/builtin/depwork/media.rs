//! media — ffprobe metadata & ffmpeg transcoding (Depwork only).
//!
//! Wraps the ffmpeg suite if installed: `media_probe` summarizes format and
//! stream metadata as JSON; `media_convert` transcodes/compresses/crops audio
//! and video. When ffmpeg/ffprobe are missing the tools return a clear error
//! telling the user how to install them (scoop/choco on Windows).
//!
//! Examples:
//! - media_probe input="C:\videos\demo.mp4"
//! - media_convert input="demo.mp4" output="demo_small.mp4" crf=28
//! - media_convert input="song.flac" output="song.mp3" audio_bitrate="128k"
//! - media_convert input="demo.mp4" output="thumb.jpg" frame=true
//! - media_convert input="demo.mp4" output="clip.mp4" start="00:01:00" duration="00:00:30"

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Scan `path_var` (a PATH-style string) for an executable named `name`.
/// Windows also tries PATHEXT-style extensions (.exe/.cmd/.bat…).
pub fn find_binary_in(path_var: &str, name: &str) -> Option<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(sep).filter(|d| !d.is_empty()) {
        let cand = Path::new(dir).join(name);
        if cand.is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            let lower = name.to_ascii_lowercase();
            if !lower.ends_with(".exe") && !lower.ends_with(".cmd") && !lower.ends_with(".bat") {
                for ext in [".exe", ".cmd", ".bat"] {
                    let with_ext = Path::new(dir).join(format!("{name}{ext}"));
                    if with_ext.is_file() {
                        return Some(with_ext);
                    }
                }
            }
        }
    }
    None
}

/// Find an executable on the system PATH.
pub fn find_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    find_binary_in(&path_var, name)
}

fn require_binary(name: &str) -> AppResult<PathBuf> {
    find_binary(name).ok_or_else(|| {
        format!(
            "{name} not found on PATH. Install FFmpeg first, e.g. `winget install ffmpeg` \
             or `scoop install ffmpeg`, then restart the app."
        )
        .into()
    })
}

fn run_capture(bin: &Path, args: &[String]) -> AppResult<(bool, String)> {
    let output = std::process::Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run {}: {e}", bin.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    Ok((output.status.success(), combined))
}

// ── media_probe ─────────────────────────────────────────────────────────────

/// Parse ffprobe JSON into a readable summary. Pure — unit-testable.
pub fn summarize_probe(raw: &str) -> AppResult<String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| format!("Invalid ffprobe output: {e}"))?;
    let mut lines: Vec<String> = Vec::new();
    if let Some(fmt) = value.get("format") {
        let name = fmt
            .get("format_long_name")
            .or_else(|| fmt.get("format_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let dur = fmt
            .get("duration")
            .and_then(|v| v.as_str())
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "?".to_string());
        let size = fmt
            .get("size")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .map(|b| format!("{:.1} MB", b / 1024.0 / 1024.0))
            .unwrap_or_else(|| "?".to_string());
        lines.push(format!("Format: {name} | duration: {dur} | size: {size}"));
    }
    if let Some(streams) = value.get("streams").and_then(|s| s.as_array()) {
        for stream in streams {
            let index = stream.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let codec_type = stream
                .get("codec_type")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let codec = stream
                .get("codec_long_name")
                .or_else(|| stream.get("codec_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let mut detail = format!("  Stream {index} [{codec_type}]: {codec}");
            if let (Some(w), Some(h)) = (
                stream.get("width").and_then(|v| v.as_u64()),
                stream.get("height").and_then(|v| v.as_u64()),
            ) {
                detail.push_str(&format!(" {w}x{h}"));
            }
            let sr = stream.get("sample_rate");
            let sr_text = match sr.and_then(|v| v.as_str()) {
                Some(s) => Some(s.to_string()),
                None => sr.and_then(|v| v.as_u64()).map(|n| n.to_string()),
            };
            if let Some(sr) = sr_text {
                detail.push_str(&format!(" {sr} Hz"));
            }
            if let Some(ch) = stream.get("channels").and_then(|v| v.as_u64()) {
                detail.push_str(&format!(" {ch}ch"));
            }
            if let Some(bit) = stream.get("bit_rate").and_then(|v| v.as_str()) {
                detail.push_str(&format!(" bitrate {bit}"));
            }
            lines.push(detail);
        }
    }
    if lines.is_empty() {
        return Err("No media information in ffprobe output".into());
    }
    Ok(lines.join("\n"))
}

/// Media metadata tool.
pub struct MediaProbeTool;

impl MediaProbeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for MediaProbeTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "media_probe"
    }

    fn description(&self) -> &str {
        "Read media metadata (duration, format, codecs, resolution, sample \
         rate) with ffprobe. Parameters: input (required, media file path). \
         Requires FFmpeg on PATH (winget/scoop install ffmpeg)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "Media file path." }
            },
            "required": ["input"]
        })
    }

    /// Pure metadata read — never prompts.
    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let input = args
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: input".to_string())?;
        let path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), input);
        if !path.is_file() {
            return Err(format!("File not found: {}", path.display()).into());
        }
        let bin = require_binary("ffprobe")?;
        let input_str = path.to_string_lossy().to_string();
        let out = tokio::task::spawn_blocking(move || {
            let args = vec![
                "-v".to_string(),
                "error".to_string(),
                "-print_format".to_string(),
                "json".to_string(),
                "-show_format".to_string(),
                "-show_streams".to_string(),
                input_str,
            ];
            let (ok, raw) = run_capture(&bin, &args)?;
            if !ok {
                return Err(raw);
            }
            summarize_probe(&raw).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("probe task panicked: {e}"))??;
        Ok(ToolResult::success(out))
    }
}

// ── media_convert ───────────────────────────────────────────────────────────

/// Convert options, kept as a plain struct so argument building is testable.
#[derive(Default)]
pub struct ConvertOptions {
    pub crf: Option<u8>,
    pub video_bitrate: Option<String>,
    pub audio_bitrate: Option<String>,
    pub start: Option<String>,
    pub duration: Option<String>,
    pub scale: Option<String>,
    pub subtitle: Option<String>,
    pub frame: bool,
}

/// Build the ffmpeg argument vector. Pure — unit-testable.
pub fn build_ffmpeg_args(input: &str, output: &str, opts: &ConvertOptions) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
    ];
    if let Some(start) = &opts.start {
        args.push("-ss".to_string());
        args.push(start.clone());
    }
    if let Some(duration) = &opts.duration {
        args.push("-t".to_string());
        args.push(duration.clone());
    }
    args.push("-i".to_string());
    args.push(input.to_string());
    if opts.frame {
        // Single-frame extraction (thumbnail) — no re-encode by default.
        args.push("-frames:v".to_string());
        args.push("1".to_string());
    }
    if let Some(crf) = opts.crf {
        args.push("-c:v".to_string());
        args.push("libx264".to_string());
        args.push("-crf".to_string());
        args.push(crf.to_string());
        args.push("-preset".to_string());
        args.push("medium".to_string());
    } else if let Some(bitrate) = &opts.video_bitrate {
        args.push("-c:v".to_string());
        args.push("libx264".to_string());
        args.push("-b:v".to_string());
        args.push(bitrate.clone());
    }
    if let Some(bitrate) = &opts.audio_bitrate {
        args.push("-c:a".to_string());
        args.push("aac".to_string());
        args.push("-b:a".to_string());
        args.push(bitrate.clone());
    }
    let mut filters: Vec<String> = Vec::new();
    if let Some(scale) = &opts.scale {
        filters.push(format!("scale={scale}"));
    }
    if let Some(subtitle) = &opts.subtitle {
        filters.push(subtitle_filter(subtitle));
    }
    if !filters.is_empty() {
        args.push("-vf".to_string());
        args.push(filters.join(","));
    }
    args.push(output.to_string());
    args
}

/// FFmpeg `subtitles=` filter value: forward slashes (Windows path
/// escaping inside the filter graph) and single quotes stripped (they
/// would break the filter quoting).
fn subtitle_filter(path: &str) -> String {
    let cleaned = path.replace('\\', "/").replace('\'', "");
    format!("subtitles='{cleaned}'")
}

/// Media conversion tool.
pub struct MediaConvertTool;

impl MediaConvertTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for MediaConvertTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "media_convert"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Transcode/compress/crop media with ffmpeg. Parameters: input \
         (required), output (required, extension drives format), crf \
         (0-51, lower = better quality, 23 default; 28+ compresses), \
         video_bitrate (e.g. \"800k\"), audio_bitrate (e.g. \"128k\"), \
                 start/duration (e.g. \"00:01:00\"), scale (e.g. \"1280:720\"), \
                 subtitle (path to .srt/.ass to burn into the video), \
                 frame (true = extract a single frame to output as jpg/png). \
         Requires FFmpeg on PATH (winget/scoop install ffmpeg)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "Source media file path." },
                "output": { "type": "string", "description": "Destination path (extension = target format)." },
                "crf": { "type": "number", "description": "H.264 quality 0-51 (28+ = strong compression)." },
                "video_bitrate": { "type": "string", "description": "Video bitrate e.g. \"800k\"." },
                "audio_bitrate": { "type": "string", "description": "Audio bitrate e.g. \"128k\"." },
                "start": { "type": "string", "description": "Start offset e.g. \"00:01:00\"." },
                  "duration": { "type": "string", "description": "Clip length e.g. \"00:00:30\"." },
                  "scale": { "type": "string", "description": "Resolution e.g. \"1280:720\"." },
                  "subtitle": { "type": "string", "description": "Subtitle file path (.srt/.ass) to burn in." },
                  "frame": { "type": "boolean", "description": "Extract a single frame (thumbnail)." }
            },
            "required": ["input", "output"]
        })
    }

    /// Self-approval: transcoding to a NEW output never prompts; own
    /// session outputs are allowed; overwriting a pre-existing user file
    /// asks. Runs after the unified pipeline's deny rules.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("output").and_then(|o| o.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target = super::permissions::resolve_target(context.workspace.as_deref(), raw, None);
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let input = args
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: input".to_string())?;
        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: output".to_string())?;
        let input_path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), input);
        if !input_path.is_file() {
            return Err(format!("File not found: {}", input_path.display()).into());
        }
        let output_path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), output);
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let opts = ConvertOptions {
            crf: args
                .get("crf")
                .and_then(|v| v.as_u64())
                .map(|c| c.min(51) as u8),
            video_bitrate: args
                .get("video_bitrate")
                .and_then(|v| v.as_str())
                .map(String::from),
            audio_bitrate: args
                .get("audio_bitrate")
                .and_then(|v| v.as_str())
                .map(String::from),
            start: args.get("start").and_then(|v| v.as_str()).map(String::from),
            duration: args
                .get("duration")
                .and_then(|v| v.as_str())
                .map(String::from),
            scale: args.get("scale").and_then(|v| v.as_str()).map(String::from),
            subtitle: args
                .get("subtitle")
                .and_then(|v| v.as_str())
                .map(String::from),
            frame: args.get("frame").and_then(|v| v.as_bool()).unwrap_or(false),
        };
        let bin = require_binary("ffmpeg")?;
        let input_str = input_path.to_string_lossy().to_string();
        let output_str = output_path.to_string_lossy().to_string();
        let out = tokio::task::spawn_blocking(move || {
            let args = build_ffmpeg_args(&input_str, &output_str, &opts);
            let (ok, output) = run_capture(&bin, &args)?;
            if !ok {
                return Err(format!("ffmpeg failed: {output}"));
            }
            let mut summary = format!("Converted {} → {}", input_str, output_str);
            if !output.trim().is_empty() {
                summary.push_str(&format!("\n{output}"));
            }
            Ok(summary)
        })
        .await
        .map_err(|e| format!("convert task panicked: {e}"))??;
        super::permissions::record_output(context, &output_path);
        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_binary_scans_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("ffmpeg");
        std::fs::write(&bin, "fake").expect("write");
        let path_var = format!(
            "{};{}",
            dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        assert_eq!(find_binary_in(&path_var, "ffmpeg"), Some(bin.clone()));
        assert_eq!(find_binary_in(&path_var, "nonexistent-tool"), None);
        assert!(find_binary_in("", "anything").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn find_binary_tries_extensions_on_windows() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("ffprobe.cmd"), "@echo fake").expect("write");
        let path_var = dir.path().display().to_string();
        let found = find_binary_in(&path_var, "ffprobe").expect("found");
        assert!(found.to_string_lossy().ends_with("ffprobe.cmd"));
    }

    #[test]
    fn probe_summary_parses_ffprobe_json() {
        let raw = r#"{
            "format": {
                "format_long_name": "QuickTime / MOV",
                "duration": "12.5",
                "size": "1048576"
            },
            "streams": [
                {
                    "index": 0,
                    "codec_type": "video",
                    "codec_long_name": "H.264 / AVC",
                    "width": 1920,
                    "height": 1080,
                    "bit_rate": "2000000"
                },
                {
                    "index": 1,
                    "codec_type": "audio",
                    "codec_long_name": "AAC (Advanced Audio Coding)",
                    "sample_rate": "48000",
                    "channels": 2
                }
            ]
        }"#;
        let out = summarize_probe(raw).expect("parse");
        assert!(out.contains("QuickTime"));
        assert!(out.contains("12.5s"));
        assert!(out.contains("1.0 MB"));
        assert!(out.contains("1920x1080"));
        assert!(out.contains("48000 Hz"));
        assert!(out.contains("2ch"));
    }

    #[test]
    fn probe_summary_rejects_garbage() {
        assert!(summarize_probe("not json").is_err());
        assert!(summarize_probe("{}").is_err());
    }

    #[test]
    fn ffmpeg_args_build_in_order() {
        let opts = ConvertOptions {
            crf: Some(28),
            start: Some("00:01:00".to_string()),
            duration: Some("00:00:30".to_string()),
            frame: false,
            ..Default::default()
        };
        let args = build_ffmpeg_args("in.mp4", "out.mp4", &opts);
        assert!(args.starts_with(&["-y".to_string()]));
        let ss = args.iter().position(|a| a == "-ss").expect("-ss");
        assert_eq!(args[ss + 1], "00:01:00");
        let i = args.iter().position(|a| a == "-i").expect("-i");
        assert_eq!(args[i + 1], "in.mp4");
        let crf = args.iter().position(|a| a == "-crf").expect("-crf");
        assert_eq!(args[crf + 1], "28");
        assert!(args.ends_with(&["out.mp4".to_string()]));
    }

    #[test]
    fn ffmpeg_args_frame_mode() {
        let opts = ConvertOptions {
            frame: true,
            ..Default::default()
        };
        let args = build_ffmpeg_args("in.mp4", "thumb.jpg", &opts);
        let frames = args
            .iter()
            .position(|a| a == "-frames:v")
            .expect("-frames:v");
        assert_eq!(args[frames + 1], "1");
        assert!(args.ends_with(&["thumb.jpg".to_string()]));
    }

    #[test]
    fn ffmpeg_args_burn_subtitles() {
        let opts = ConvertOptions {
            subtitle: Some(r"C:\videos\subs\my sub.srt".to_string()),
            ..Default::default()
        };
        let args = build_ffmpeg_args("in.mp4", "out.mp4", &opts);
        let vf = args.iter().position(|a| a == "-vf").expect("-vf");
        assert_eq!(
            args[vf + 1],
            "subtitles='C:/videos/subs/my sub.srt'",
            "backslashes normalized, quotes stripped"
        );
    }

    #[test]
    fn ffmpeg_args_combine_scale_and_subtitles_in_one_vf() {
        let opts = ConvertOptions {
            scale: Some("1280:720".to_string()),
            subtitle: Some("sub.srt".to_string()),
            ..Default::default()
        };
        let args = build_ffmpeg_args("in.mp4", "out.mp4", &opts);
        let vf = args.iter().position(|a| a == "-vf").expect("-vf");
        assert_eq!(
            args[vf + 1],
            "scale=1280:720,subtitles='sub.srt'",
            "one -vf graph with both filters"
        );
        assert_eq!(args.iter().filter(|a| *a == "-vf").count(), 1);
    }
}
