//! chart_generate — render bar/line/pie charts as SVG (Depwork only).
//!
//! Pure-Rust chart rendering with no external binaries. Input is a plain data
//! table: labels + one or more named series. Output is an .svg file that can
//! be embedded in reports, opened in a browser or converted later.
//!
//! Data format:
//! {
//!   "labels": ["Q1", "Q2", "Q3"],
//!   "series": [
//!     { "name": "收入", "values": [10, 20, 30] },
//!     { "name": "成本", "values": [5, 8, 12] }
//!   ]
//! }
//! For pie charts a single series is used.
//!
//! Example:
//! - chart_generate kind="bar" output="sales.svg" data={...}
#![allow(clippy::write_with_newline)]

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fmt::Write as _;

const PALETTE: [&str; 8] = [
    "#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7",
];
const MARGIN: (f64, f64, f64, f64) = (60.0, 20.0, 60.0, 20.0); // left, top, right, bottom

/// Chart input model.
pub struct ChartData {
    pub labels: Vec<String>,
    pub series: Vec<ChartSeries>,
}

pub struct ChartSeries {
    pub name: String,
    pub values: Vec<f64>,
}

pub fn parse_chart_data(value: &Value) -> AppResult<ChartData> {
    let labels: Vec<String> = value
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let series: Vec<ChartSeries> = value
        .get("series")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let name = s
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("series")
                        .to_string();
                    let values: Vec<f64> = s
                        .get("values")
                        .and_then(|v| v.as_array())
                        .map(|vs| {
                            vs.iter()
                                .filter_map(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (!values.is_empty()).then_some(ChartSeries { name, values })
                })
                .collect()
        })
        .unwrap_or_default();
    if labels.is_empty() {
        return Err("data.labels must be a non-empty array of strings".into());
    }
    if series.is_empty() {
        return Err("data.series must be a non-empty array of {name, values}".into());
    }
    Ok(ChartData { labels, series })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn max_value(data: &ChartData) -> f64 {
    data.series
        .iter()
        .flat_map(|s| s.values.iter())
        .fold(0.0_f64, |m, v| m.max(*v))
        .max(1.0)
}

/// Render a chart to an SVG string. Pure — unit-testable.
pub fn render_chart(kind: &str, data: &ChartData, width: u32, height: u32) -> AppResult<String> {
    match kind {
        "bar" => Ok(render_bar(data, width, height)),
        "line" => Ok(render_line(data, width, height)),
        "pie" => Ok(render_pie(data, width, height)),
        other => Err(format!("Unknown chart kind: {other}. Use bar/line/pie").into()),
    }
}

fn render_bar(data: &ChartData, width: u32, height: u32) -> String {
    let (w, h) = (width as f64, height as f64);
    let (pl, pt, pb, pr) = MARGIN;
    let plot_w = w - pl - pr;
    let plot_h = h - pt - pb;
    let max_v = max_value(data);
    let n = data.labels.len();
    let s_count = data.series.len();
    let group_w = plot_w / n as f64;
    let bar_w = group_w * 0.8 / s_count as f64;

    let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 "#);
    let _ = write!(
        svg,
        "{w} {h}\" font-family=\"Segoe UI, Arial, sans-serif\">\n"
    );
    let _ = write!(
        svg,
        "  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"white\"/>\n"
    );
    // Y gridlines + labels (5 lines).
    for i in 0..=5 {
        let frac = i as f64 / 5.0;
        let y = pt + plot_h * (1.0 - frac);
        let val = max_v * frac;
        let _ = write!(
            svg,
            "  <line x1=\"{pl}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"#e5e5e5\" stroke-width=\"1\"/>\n",
            w - pr
        );
        let label = format_val(val);
        let _ = write!(
            svg,
            "  <text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#666\" text-anchor=\"end\">{}</text>\n",
            pl - 6.0,
            y + 4.0,
            esc(&label)
        );
    }
    // Bars.
    for (si, series) in data.series.iter().enumerate() {
        let color = PALETTE[si % PALETTE.len()];
        for (li, value) in series.values.iter().enumerate() {
            let group_x = pl + li as f64 * group_w;
            let bar_h = (value / max_v).max(0.0) * plot_h;
            let x = group_x + group_w * 0.1 + si as f64 * bar_w;
            let y = pt + plot_h - bar_h;
            let _ = write!(
                svg,
                "  <rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{:.1}\" height=\"{bar_h:.1}\" fill=\"{color}\"/>\n",
                bar_w - 1.0
            );
        }
    }
    // X labels.
    for (li, label) in data.labels.iter().enumerate() {
        let cx = pl + li as f64 * group_w + group_w / 2.0;
        let _ = write!(
            svg,
            "  <text x=\"{cx:.1}\" y=\"{}\" font-size=\"11\" fill=\"#333\" text-anchor=\"middle\">{}</text>\n",
            pt + plot_h + 18.0,
            esc(label)
        );
    }
    svg.push_str(&legend(data));
    svg.push_str("</svg>\n");
    svg
}

fn render_line(data: &ChartData, width: u32, height: u32) -> String {
    let (w, h) = (width as f64, height as f64);
    let (pl, pt, pb, pr) = MARGIN;
    let plot_w = w - pl - pr;
    let plot_h = h - pt - pb;
    let max_v = max_value(data);
    let n = data.labels.len().max(2) as f64;

    let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 "#);
    let _ = write!(
        svg,
        "{w} {h}\" font-family=\"Segoe UI, Arial, sans-serif\">\n"
    );
    let _ = write!(
        svg,
        "  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"white\"/>\n"
    );
    for i in 0..=5 {
        let frac = i as f64 / 5.0;
        let y = pt + plot_h * (1.0 - frac);
        let val = max_v * frac;
        let _ = write!(
            svg,
            "  <line x1=\"{pl}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"#e5e5e5\" stroke-width=\"1\"/>\n",
            w - pr
        );
        let _ = write!(
            svg,
            "  <text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#666\" text-anchor=\"end\">{}</text>\n",
            pl - 6.0,
            y + 4.0,
            esc(&format_val(val))
        );
    }
    for (si, series) in data.series.iter().enumerate() {
        let color = PALETTE[si % PALETTE.len()];
        let pts: Vec<(f64, f64)> = series
            .values
            .iter()
            .enumerate()
            .map(|(li, value)| {
                let x = pl + li as f64 * (plot_w / (n - 1.0));
                let y = pt + plot_h - (value / max_v) * plot_h;
                (x, y)
            })
            .collect();
        let poly: String = pts
            .iter()
            .map(|(x, y)| format!("{x:.1},{y:.1}"))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(
            svg,
            "  <polyline points=\"{poly}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\"/>\n"
        );
        for (x, y) in &pts {
            let _ = write!(
                svg,
                "  <circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"3\" fill=\"{color}\"/>\n"
            );
        }
    }
    for (li, label) in data.labels.iter().enumerate() {
        let x = pl + li as f64 * (plot_w / (n - 1.0));
        let _ = write!(
            svg,
            "  <text x=\"{x:.1}\" y=\"{}\" font-size=\"11\" fill=\"#333\" text-anchor=\"middle\">{}</text>\n",
            pt + plot_h + 18.0,
            esc(label)
        );
    }
    svg.push_str(&legend(data));
    svg.push_str("</svg>\n");
    svg
}

fn render_pie(data: &ChartData, width: u32, height: u32) -> String {
    let (w, h) = (width as f64, height as f64);
    let (pl, pt, pr, pb) = MARGIN;
    let r = (w - pl - pr).min(h - pt - pb) / 2.0 - 8.0;
    let cx = pl + (w - pl - pr) / 2.0;
    let cy = pt + (h - pt - pb) / 2.0;
    let series = &data.series[0];
    let total: f64 = series.values.iter().sum();
    let total = if total <= 0.0 { 1.0 } else { total };

    let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 "#);
    let _ = write!(
        svg,
        "{w} {h}\" font-family=\"Segoe UI, Arial, sans-serif\">\n"
    );
    let _ = write!(
        svg,
        "  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"white\"/>\n"
    );
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (si, value) in series.values.iter().enumerate() {
        let frac = value / total;
        if frac <= 0.0 {
            continue;
        }
        let sweep = frac * std::f64::consts::TAU;
        let end = angle + sweep;
        let (x1, y1) = (cx + r * angle.cos(), cy + r * angle.sin());
        let (x2, y2) = (cx + r * end.cos(), cy + r * end.sin());
        let large = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let color = PALETTE[si % PALETTE.len()];
        let _ = write!(
            svg,
            "  <path d=\"M {cx:.1} {cy:.1} L {x1:.1} {y1:.1} A {r:.1} {r:.1} 0 {large} 1 {x2:.1} {y2:.1} Z\" fill=\"{color}\" stroke=\"white\" stroke-width=\"1\"/>\n"
        );
        let label_angle = angle + sweep / 2.0;
        let lx = cx + r * 0.65 * label_angle.cos();
        let ly = cy + r * 0.65 * label_angle.sin();
        let label = data
            .labels
            .get(si)
            .cloned()
            .unwrap_or_else(|| format!("{}", si + 1));
        let _ = write!(
            svg,
            "  <text x=\"{lx:.1}\" y=\"{ly:.1}\" font-size=\"11\" fill=\"white\" text-anchor=\"middle\" font-weight=\"bold\">{}</text>\n",
            esc(&label)
        );
        angle = end;
    }
    svg.push_str(&legend(data));
    svg.push_str("</svg>\n");
    svg
}

fn legend(data: &ChartData) -> String {
    let mut out = String::new();
    for (si, series) in data.series.iter().enumerate() {
        let color = PALETTE[si % PALETTE.len()];
        let x = 20.0 + si as f64 * 190.0;
        let _ = write!(
            out,
            "  <rect x=\"{x}\" y=\"10\" width=\"12\" height=\"12\" fill=\"{color}\"/>\n\
             \x20 <text x=\"{}\" y=\"20\" font-size=\"12\" fill=\"#333\">{}</text>\n",
            x + 16.0,
            esc(&series.name)
        );
    }
    out
}

fn format_val(v: f64) -> String {
    if v.fract().abs() < 1e-9 && v.abs() < 1e9 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// Chart rendering tool.
pub struct ChartGenerateTool;

impl ChartGenerateTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ChartGenerateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "chart_generate"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Render a chart to an SVG file (no external dependencies). Parameters: \
         kind (bar|line|pie, required), output (required, .svg path), \
         data (required: {\"labels\": [...], \"series\": [{\"name\", \"values\"}]}), \
         width/height (optional, default 800x500). Multi-series for bar/line, \
         single series for pie. SVG can be embedded in reports or opened in a browser."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["bar", "line", "pie"],
                    "description": "Chart type."
                },
                "output": {
                    "type": "string",
                    "description": "Output .svg file path."
                },
                "data": {
                    "type": "object",
                    "description": "{\"labels\": [\"Q1\",\"Q2\"], \"series\": [{\"name\": \"收入\", \"values\": [10, 20]}]}"
                },
                "width": { "type": "number", "description": "SVG width in px (default 800)." },
                "height": { "type": "number", "description": "SVG height in px (default 500)." }
            },
            "required": ["kind", "output", "data"]
        })
    }

    /// Self-approval: creating a NEW file (or overwriting this session's
    /// own draft) skips the prompt; touching a pre-existing user file asks.
    /// Runs after the unified pipeline's deny rules — it can only lift Ask.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("output").and_then(|o| o.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target = super::permissions::resolve_target(context.workspace.as_deref(), raw, None);
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let kind = args
            .get("kind")
            .and_then(|k| k.as_str())
            .ok_or_else(|| "Missing required parameter: kind".to_string())?;
        let output = args
            .get("output")
            .and_then(|o| o.as_str())
            .ok_or_else(|| "Missing required parameter: output".to_string())?;
        let data_value = args
            .get("data")
            .ok_or_else(|| "Missing required parameter: data".to_string())?;
        let width = args
            .get("width")
            .and_then(|v| v.as_u64())
            .unwrap_or(800)
            .clamp(300, 2000) as u32;
        let height = args
            .get("height")
            .and_then(|v| v.as_u64())
            .unwrap_or(500)
            .clamp(300, 2000) as u32;

        let data = parse_chart_data(data_value)?;
        let svg = render_chart(kind, &data, width, height)?;
        let output_path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), output);
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(&output_path, &svg)?;
        super::permissions::record_output(context, &output_path);

        let points: usize = data.series.iter().map(|s| s.values.len()).sum();
        let mut summary = format!(
            "Wrote {kind} chart ({width}x{height}, {} series, {points} points) to {}",
            data.series.len(),
            output_path.display()
        );
        if !data.series.is_empty() {
            let max = max_value(&data);
            summary.push_str(&format!("\nData range: 0 – {max}"));
        }
        Ok(ToolResult::success(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChartData {
        ChartData {
            labels: vec!["Q1".into(), "Q2".into(), "Q3".into()],
            series: vec![
                ChartSeries {
                    name: "收入".into(),
                    values: vec![10.0, 25.0, 15.0],
                },
                ChartSeries {
                    name: "成本".into(),
                    values: vec![5.0, 8.0, 12.0],
                },
            ],
        }
    }

    #[test]
    fn parse_data_from_json() {
        let v = json!({
            "labels": ["Q1", "Q2"],
            "series": [
                {"name": "a", "values": [1, 2, 3]},
                {"name": "b", "values": [4, 5]}
            ]
        });
        let data = parse_chart_data(&v).expect("parse");
        assert_eq!(data.labels, vec!["Q1", "Q2"]);
        assert_eq!(data.series.len(), 2);
        assert_eq!(data.series[0].values, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn parse_data_rejects_bad_input() {
        assert!(parse_chart_data(&json!({})).is_err());
        assert!(parse_chart_data(&json!({"labels": [], "series": []})).is_err());
        assert!(parse_chart_data(
            &json!({"labels": ["a"], "series": [{"name": "x", "values": []}]})
        )
        .is_err());
    }

    #[test]
    fn bar_chart_renders_rects_and_labels() {
        let svg = render_chart("bar", &sample(), 800, 500).expect("render");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<rect"));
        assert!(svg.contains("Q1"));
        assert!(svg.contains("收入"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn line_chart_renders_polylines() {
        let svg = render_chart("line", &sample(), 800, 500).expect("render");
        assert!(svg.contains("<polyline"));
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn pie_chart_renders_paths() {
        let data = ChartData {
            labels: vec!["A".into(), "B".into()],
            series: vec![ChartSeries {
                name: "share".into(),
                values: vec![70.0, 30.0],
            }],
        };
        let svg = render_chart("pie", &data, 600, 400).expect("render");
        assert!(svg.contains("<path"));
        assert!(svg.contains("share"));
    }

    #[test]
    fn unknown_kind_rejected() {
        assert!(render_chart("scatter", &sample(), 800, 500).is_err());
    }

    #[test]
    fn svg_escapes_user_text() {
        let data = ChartData {
            labels: vec!["<A&B>".into()],
            series: vec![ChartSeries {
                name: "x<y".into(),
                values: vec![1.0],
            }],
        };
        let svg = render_chart("bar", &data, 400, 300).expect("render");
        assert!(!svg.contains("<A&B>"));
        assert!(svg.contains("&lt;A&amp;B&gt;"));
    }
}
