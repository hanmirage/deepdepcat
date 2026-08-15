//! card_generate — 图文卡片（自媒体的封面/分享卡）。
//!
//! Generates a self-contained styled HTML card (title, subtitle, bullet
//! points, optional image, brand accent) that opens in any browser and can
//! be screenshotted / printed — the 自媒体域 "封面/图文卡片" gap.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build the standalone HTML card. Pure + unit-testable.
pub fn build_card_html(
    title: &str,
    subtitle: &str,
    bullets: &[String],
    accent: &str,
    image: Option<&str>,
) -> String {
    let accent = accent.trim_start_matches('#');
    let accent = if accent.len() == 6 && accent.chars().all(|c| c.is_ascii_hexdigit()) {
        accent
    } else {
        "4F81BD"
    };
    let bullet_items: String = bullets
        .iter()
        .filter(|b| !b.trim().is_empty())
        .map(|b| format!("<li>{}</li>", xml_escape(b)))
        .collect();
    let image_html = match image {
        Some(src) if !src.trim().is_empty() => {
            format!(
                "<img src=\"{}\" alt=\"\" style=\"width:100%;max-height:360px;object-fit:cover;border-radius:12px;margin-bottom:16px;\"/>",
                xml_escape(src)
            )
        }
        _ => String::new(),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>{title}</title>
<style>
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:linear-gradient(135deg,#0f172a,#1e293b);font-family:-apple-system,'PingFang SC','Microsoft YaHei',sans-serif;padding:24px;}}
.card{{max-width:720px;width:100%;background:#ffffff;border-radius:20px;padding:32px;box-shadow:0 20px 60px rgba(0,0,0,.35);}}
h1{{font-size:28px;line-height:1.35;margin:0 0 10px;color:#0f172a;border-left:6px solid #{accent};padding-left:14px;}}
p.sub{{font-size:15px;color:#475569;margin:0 0 18px;}}
ul{{margin:0;padding-left:20px;}}
li{{font-size:15px;color:#334155;line-height:1.8;}}
</style></head>
<body><div class="card">
{image_html}
<h1>{title}</h1>
{subtitle_html}
<ul>{bullet_items}</ul>
</div></body></html>"#,
        title = xml_escape(title),
        subtitle_html = if subtitle.trim().is_empty() {
            String::new()
        } else {
            format!("<p class=\"sub\">{}</p>", xml_escape(subtitle))
        },
        accent = accent,
    )
}

/// 图文卡片生成工具（Depwork 域）。
pub struct CardGenerateTool;

impl CardGenerateTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CardGenerateTool {
    fn name(&self) -> &str {
        "card_generate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Generate a 图文卡片 (social-card HTML): title, subtitle, bullet \
         points, optional image and brand accent color. Opens in any \
         browser; screenshot/print it for covers, shares and posters. \
         Parameters: path (required, .html), title, subtitle (optional), \
         bullets (optional array), accent (optional #RRGGBB), image \
         (optional file path referenced by the card)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Output HTML path (adds .html if missing)."},
                "title": {"type": "string", "description": "Card title."},
                "subtitle": {"type": "string", "description": "Optional subtitle."},
                "bullets": {"type": "array", "items": {"type": "string"}, "description": "Optional bullet points."},
                "accent": {"type": "string", "description": "Brand accent color #RRGGBB (default #4F81BD)."},
                "image": {"type": "string", "description": "Optional image path referenced by the card."}
            },
            "required": ["path", "title"]
        })
    }

    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("path").and_then(|p| p.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("html"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_raw = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let title = args
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            return Err("Missing required parameter: title".into());
        }
        let subtitle = args.get("subtitle").and_then(|s| s.as_str()).unwrap_or("");
        let bullets: Vec<String> = args
            .get("bullets")
            .and_then(|b| b.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let accent = args
            .get("accent")
            .and_then(|a| a.as_str())
            .unwrap_or("#4F81BD");
        let image = args.get("image").and_then(|i| i.as_str());

        let html = build_card_html(title, subtitle, &bullets, accent, image);
        let mut path = super::permissions::resolve_target(
            context.workspace.as_deref(),
            path_raw,
            Some("html"),
        );
        if path.extension().is_none() {
            path.set_extension("html");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("Failed to create output dir: {e}")))?;
        }
        tokio::fs::write(&path, html.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("Failed to write {}: {e}", path.display())))?;
        super::permissions::record_output(context, &path);
        Ok(crate::toolkit::ToolResult::success(format!(
            "已生成图文卡片：{}\n用浏览器打开即可查看/截图。",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_html_contains_title_bullets_and_accent() {
        let html = build_card_html(
            "Q3 增长 15%",
            "市场简报",
            &["营收 +15%".to_string(), "用户 +2.1 万".to_string()],
            "#112233",
            Some("cover.png"),
        );
        assert!(html.contains("Q3 增长 15%"));
        assert!(html.contains("市场简报"));
        assert!(html.contains("营收 +15%"));
        assert!(html.contains("border-left:6px solid #112233"));
        assert!(html.contains("cover.png"));
    }

    #[test]
    fn card_html_escapes_user_text() {
        let html = build_card_html(
            "a < b & c",
            "",
            &["<script>alert(1)</script>".to_string()],
            "invalid",
            None,
        );
        assert!(html.contains("a &lt; b &amp; c"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("#4F81BD"), "invalid accent falls back");
    }
}
