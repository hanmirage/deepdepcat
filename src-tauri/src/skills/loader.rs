//! Skill loader — loads skills from bundled, file, and ecosystem sources.
//!
//! Ecosystem compatibility (P1-8, "don't build your own skills market"):
//! `.claude/skills` directories (user-level and project-level) are scanned
//! as equivalent `SKILL.md` sources, gated by
//! `DEEPDEPCAT_CLAUDE_SKILLS_ENABLED` (default on). Own sources always win
//! name collisions; vendor-shipped default skills are blacklisted; SKILL.md
//! frontmatter is parsed with three-tier tolerance so a broken YAML header
//! never loses the skill.

use crate::core::error::{AppError, AppResult};
use crate::skills::bundled;
use crate::skills::types::{
    is_vendor_default_skill, Skill, SkillSource, MAX_BODY_PEEK_BYTES, MAX_DESCRIPTION_LEN,
    MAX_FRONTMATTER_BYTES, MAX_NAME_LEN,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A directory qualifies as a Claude/Cursor plugin when it carries a plugin
/// manifest (`plugin.json` / `.claude-plugin/plugin.json`) or any recognizable
/// plugin payload (`skills/`, `agents/`, `.mcp.json`, `hooks/hooks.json`, or
/// a bare `SKILL.md` which gets a synthesized manifest).
fn is_plugin_dir(path: &Path) -> bool {
    path.join("plugin.json").is_file()
        || path.join(".claude-plugin").join("plugin.json").is_file()
        || path.join("skills").is_dir()
        || path.join("agents").is_dir()
        || path.join(".mcp.json").is_file()
        || path.join("hooks").join("hooks.json").is_file()
        || path.join("SKILL.md").is_file()
}

/// The skill loader — discovers and loads skills from all sources.
pub struct SkillLoader {
    skills_dir: PathBuf,
    /// Current workspace — enables project-level `.deepdepcat/skills` and
    /// ecosystem `.claude/skills`.
    workspace: Option<PathBuf>,
    /// Claude ecosystem compat gate (from `[skills]` config; env overrides).
    claude_enabled: bool,
}

impl SkillLoader {
    pub fn new(app_data_dir: &std::path::Path) -> Self {
        let skills_dir = app_data_dir.join("skills");
        Self {
            skills_dir,
            workspace: None,
            claude_enabled: true,
        }
    }

    /// Attach the current workspace (project-level ecosystem skills).
    pub fn with_workspace(mut self, workspace: Option<PathBuf>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Set the Claude ecosystem compat gate (from `[skills]` config section).
    pub fn with_compat(mut self, claude_enabled: bool) -> Self {
        self.claude_enabled = claude_enabled;
        self
    }

    /// Whether the Claude ecosystem source is enabled. Priority: env gate
    /// (`DEEPDEPCAT_CLAUDE_SKILLS_ENABLED`, default on) > `[skills]` config gate.
    fn eco_enabled(&self) -> bool {
        match std::env::var("DEEPDEPCAT_CLAUDE_SKILLS_ENABLED") {
            Ok(v) => !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            ),
            Err(_) => self.claude_enabled,
        }
    }

    /// Load all available skills.
    pub fn load_all(&self) -> AppResult<Vec<Skill>> {
        let mut skills = Vec::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        let mut seen_names: HashSet<String> = HashSet::new();

        // 1. Bundled skills.
        skills.extend(bundled::get_bundled_skills());

        // 2. Own flat files (legacy layout: app_data_dir/skills/*.md).
        self.load_flat_dir(
            &self.skills_dir,
            &mut skills,
            &mut seen_paths,
            &mut seen_names,
        );

        // 3. Own directory layout (`<dir>/skills/<name>/SKILL.md`) — own
        // sources first so they win name collisions (first-seen wins).
        let mut own_dirs = vec![
            self.skills_dir.clone(),
            crate::workspace::project_files::user_deepdepcat_dir().join("skills"),
        ];
        if let Some(ws) = &self.workspace {
            own_dirs.push(ws.join(".deepdepcat").join("skills"));
        }
        for dir in own_dirs {
            self.load_skill_dirs(&dir, &mut skills, &mut seen_paths, &mut seen_names);
        }
        // 3b. Mode-organized user skills — `~/.deepdepcat/{depwork,code}/skills/
        // <name>/SKILL.md`. The directory name supplies the work mode for
        // skills that do not declare it.
        {
            let user_ddc = crate::workspace::project_files::user_deepdepcat_dir();
            for mode in ["depwork", "code"] {
                self.load_skill_dirs_as(
                    &user_ddc.join(mode).join("skills"),
                    SkillSource::File,
                    Some(mode),
                    &mut skills,
                    &mut seen_paths,
                    &mut seen_names,
                );
            }
        }

        // 4. Ecosystem user-level skills (gated, default on).
        if let Some(home) = dirs::home_dir() {
            if self.eco_enabled() {
                self.load_skill_dirs(
                    &home.join(".claude").join("skills"),
                    &mut skills,
                    &mut seen_paths,
                    &mut seen_names,
                );
            }
        }

        // 5. Ecosystem project-level skills (gated).
        if let Some(ws) = &self.workspace {
            if self.eco_enabled() {
                self.load_skill_dirs(
                    &ws.join(".claude").join("skills"),
                    &mut skills,
                    &mut seen_paths,
                    &mut seen_names,
                );
            }
        }

        // 6. Claude/Cursor plugin layout (P1-8, lowest source priority).
        // Plugins carry `skills/` payloads (or a bare SKILL.md with a
        // synthesized manifest); we eat the layout directly. The plugin's
        // skills are scanned through the same canonical-dedup path so
        // name collisions always resolve in favor of higher-priority sources.
        if let Some(home) = dirs::home_dir() {
            if self.eco_enabled() {
                self.load_plugin_roots(
                    &home.join(".claude").join("plugins"),
                    &mut skills,
                    &mut seen_paths,
                    &mut seen_names,
                );
            }
        }
        if let Some(ws) = &self.workspace {
            if self.eco_enabled() {
                self.load_plugin_roots(
                    &ws.join(".claude").join("plugins"),
                    &mut skills,
                    &mut seen_paths,
                    &mut seen_names,
                );
            }
        }

        Ok(skills)
    }

    /// Scan a flat directory of `*.md` skill files (own legacy layout).
    fn load_flat_dir(
        &self,
        dir: &Path,
        skills: &mut Vec<Skill>,
        seen_paths: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let name = normalize_name(name);
            if name.is_empty() || !seen_names.insert(name.clone()) {
                continue;
            }
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen_paths.insert(canonical) {
                continue;
            }
            if let Some(skill) = parse_flat_skill(&path, &name) {
                skills.push(skill);
            }
        }
    }

    /// Scan `<dir>/<name>/SKILL.md` directories (own + ecosystem layout).
    fn load_skill_dirs(
        &self,
        dir: &Path,
        skills: &mut Vec<Skill>,
        seen_paths: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<String>,
    ) {
        self.load_skill_dirs_as(dir, SkillSource::File, None, skills, seen_paths, seen_names)
    }

    /// `load_skill_dirs` with an explicit source tag (plugins → Plugin) and an
    /// optional mode hint — skills scanned from a mode-organized directory
    /// (`~/.deepdepcat/{depwork,code}/skills/`) inherit that mode when their
    /// frontmatter does not declare `work_mode`.
    fn load_skill_dirs_as(
        &self,
        dir: &Path,
        source: SkillSource,
        mode_hint: Option<&str>,
        skills: &mut Vec<Skill>,
        seen_paths: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Vendor-shipped default skills never leak in (dual condition:
            // vendor path segment + matching name).
            if is_vendor_default_skill(&skill_md, dir_name) {
                continue;
            }
            let canonical = skill_md.canonicalize().unwrap_or_else(|_| skill_md.clone());
            if !seen_paths.insert(canonical) {
                continue;
            }
            if let Some(mut skill) = parse_skill_dir_skill(&skill_md, dir_name) {
                skill.source = source.clone();
                // Mode inference: a skill under `depwork/skills/` or
                // `code/skills/` belongs to that mode when its frontmatter
                // does not declare `work_mode` itself.
                if skill.work_modes.is_empty() {
                    if let Some(mode) = mode_hint {
                        skill.work_modes = vec![mode.to_string()];
                    }
                }
                if seen_names.insert(skill.name.clone()) {
                    skills.push(skill);
                }
            }
        }
    }

    /// Scan a plugin root (e.g. `~/.claude/plugins` or `<ws>/.claude/plugins`)
    /// for Claude/Cursor plugin directories. Each plugin's `skills/` payload
    /// is loaded with the lowest-priority `Plugin` source tag. A plugin that
    /// is itself a bare SKILL.md directory gets a synthesized manifest (its
    /// directory name is the skill name). Canonical prefix checks guard
    /// against `../../` and symlink escapes.
    fn load_plugin_roots(
        &self,
        root: &Path,
        skills: &mut Vec<Skill>,
        seen_paths: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<String>,
    ) {
        let Ok(root_canonical) = root.canonicalize() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(canon) = path.canonicalize() else {
                continue;
            };
            if !canon.starts_with(&root_canonical) {
                continue; // symlink escape guard
            }
            if !is_plugin_dir(&path) {
                continue;
            }
            let skills_dir = path.join("skills");
            if skills_dir.is_dir() {
                self.load_skill_dirs_as(
                    &skills_dir,
                    SkillSource::Plugin,
                    None,
                    skills,
                    seen_paths,
                    seen_names,
                );
            } else if path.join("SKILL.md").is_file() {
                // Bare SKILL.md plugin — the plugin dir itself is the skill
                // (manifest synthesized from the directory name).
                let skill_md = path.join("SKILL.md");
                let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if is_vendor_default_skill(&skill_md, dir_name) {
                    continue;
                }
                let canonical = skill_md.canonicalize().unwrap_or_else(|_| skill_md.clone());
                if !seen_paths.insert(canonical) {
                    continue;
                }
                if let Some(mut skill) = parse_skill_dir_skill(&skill_md, dir_name) {
                    skill.source = SkillSource::Plugin;
                    if seen_names.insert(skill.name.clone()) {
                        skills.push(skill);
                    }
                }
            }
        }
    }

    /// Save a skill to a file.
    pub fn save_skill(&self, skill: &Skill) -> AppResult<()> {
        // Traversal guard: the id comes from frontend IPC and becomes a file
        // name under skills_dir — it must never carry path separators or
        // `..` (that would write outside the skills directory).
        if !validate_skill_id(&skill.id) {
            return Err(AppError::Path(format!(
                "Invalid skill id {:?} — only letters, digits, '-' and '_' are allowed",
                skill.id
            )));
        }
        std::fs::create_dir_all(&self.skills_dir)?;

        let filename = format!("{}.md", skill.id);
        let path = self.skills_dir.join(filename);

        let mut content = String::new();

        // Write frontmatter
        content.push_str("---\n");
        content.push_str(&format!("name: {}\n", skill.name));
        content.push_str(&format!("description: {}\n", skill.description));
        if let Some(ref model) = skill.model {
            content.push_str(&format!("model: {}\n", model));
        }
        if !skill.allowed_tools.is_empty() {
            content.push_str(&format!(
                "allowed_tools: {}\n",
                skill.allowed_tools.join(", ")
            ));
        }
        if let Some(ref mode) = skill.permission_mode {
            content.push_str(&format!("permission_mode: {}\n", mode));
        }
        if !skill.paths.is_empty() {
            content.push_str(&format!("paths: {}\n", skill.paths.join(", ")));
        }
        if !skill.work_modes.is_empty() {
            content.push_str(&format!("work_mode: {}\n", skill.work_modes.join(", ")));
        }
        content.push_str("---\n\n");

        // Write body
        content.push_str(&skill.content);

        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Delete a skill file.
    pub fn delete_skill(&self, skill_id: &str) -> AppResult<()> {
        // Traversal guard — same contract as `save_skill`: the id must never
        // escape the skills directory.
        if !validate_skill_id(skill_id) {
            return Err(AppError::Path(format!(
                "Invalid skill id {:?} — only letters, digits, '-' and '_' are allowed",
                skill_id
            )));
        }
        let path = self.skills_dir.join(format!("{}.md", skill_id));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// Validate a skill id before it is used as a file name: only slug-safe
/// characters, no path separators, no `.`/`..` escapes.
fn validate_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Normalize a skill name to a slug (upper-case / underscore / dot / space
/// → `-`, consecutive separators collapsed), capped at `MAX_NAME_LEN`.
fn normalize_name(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in raw.trim().chars() {
        if c.is_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_sep = true;
        }
    }
    out.chars().take(MAX_NAME_LEN).collect()
}

/// Derive a description from the body when frontmatter lacks one: the first
/// prose line (heading markers stripped), capped at `MAX_DESCRIPTION_LEN`.
fn derive_description(body: &str) -> String {
    for line in body.lines().take(MAX_BODY_PEEK_BYTES) {
        let line = line.trim().trim_start_matches('#').trim();
        if !line.is_empty() && !line.starts_with("```") {
            return line.chars().take(MAX_DESCRIPTION_LEN).collect();
        }
    }
    String::new()
}

/// Three-tier tolerant frontmatter parse for a SKILL.md.
///
/// 1. Strict YAML (`serde_yaml`) — gated by structural limits so hostile
///    frontmatter (deeply nested or oversized YAML) can't exhaust the parser;
///    out-of-limit frontmatter falls through to the scalar rescue instead.
/// 2. Line-by-line scalar rescue (existing behavior) — list/map fields are
///    never reconstructed from broken YAML, but the skill is never dropped.
fn parse_skill_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    // Normalize CRLF → LF first: Windows-authored SKILL.md (Notepad, git
    // autocrlf) uses "---\r\n", and strip_prefix("---\n") would fail — the
    // whole file then becomes body and every metadata field is lost.
    let content = normalize_line_endings(content);
    let Some(rest) = content.strip_prefix("---\n") else {
        return (HashMap::new(), content.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        // No closing fence — treat whole file as body (tolerate broken files).
        return (HashMap::new(), content.to_string());
    };
    let fm_str = &rest[..end.min(MAX_FRONTMATTER_BYTES)];
    // `end` points at the `\n` of the closing `\n---` fence (4 bytes). The
    // body starts at `end + 4` — NOT `end + 5` — because the fence may be the
    // file's last line with no trailing newline, in which case `end + 5` is
    // one byte past the end and panics. Strip a single leading `\n` when the
    // fence DID carry a trailing newline (the common `\n---\n` shape).
    let body = rest
        .get(end + 4..)
        .map(|b| b.strip_prefix('\n').unwrap_or(b))
        .unwrap_or("")
        .to_string();

    // Tier 1: strict YAML mapping of scalars.
    let mut map: HashMap<String, String> = HashMap::new();
    if frontmatter_within_limits(fm_str) {
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(fm_str) {
            if let serde_yaml::Value::Mapping(mapping) = value {
                for (k, v) in mapping.into_iter().take(MAX_FRONTMATTER_KEYS) {
                    // Normalize kebab-case to snake_case — skill files write
                    // `allowed-tools` / `when-to-use`, the loader reads
                    // `allowed_tools` / `when_to_use`.
                    let key = k.as_str().unwrap_or("").replace('-', "_");
                    let val = match v {
                        serde_yaml::Value::String(s) => s,
                        serde_yaml::Value::Sequence(seq) => seq
                            .iter()
                            .filter_map(|i| i.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                            .join(", "),
                        serde_yaml::Value::Bool(b) => b.to_string(),
                        serde_yaml::Value::Number(n) => n.to_string(),
                        serde_yaml::Value::Null => String::new(),
                        other => format!("{other:?}"),
                    };
                    map.insert(key, val);
                }
            }
            return (map, body);
        }
    }

    // Tier 2: line-by-line scalar rescue.
    for line in fm_str.lines() {
        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim().to_string();
            let value = line[colon_pos + 1..].trim().to_string();
            map.insert(key, value);
        }
    }
    (map, body)
}

/// Maximum YAML nesting depth accepted in skill frontmatter.
const MAX_FRONTMATTER_DEPTH: usize = 16;
/// Maximum number of top-level keys accepted in skill frontmatter.
const MAX_FRONTMATTER_KEYS: usize = 32;

/// Structural guard run BEFORE the YAML parse: a cheap bracket-depth scan
/// rejects pathological nesting so the YAML parser never sees hostile input.
/// Bracket chars inside quoted strings only overestimate depth — the limit
/// is generous enough that legitimate frontmatter never trips it.
fn frontmatter_within_limits(fm: &str) -> bool {
    let mut depth = 0usize;
    for c in fm.chars() {
        match c {
            '[' | '{' => {
                depth += 1;
                if depth > MAX_FRONTMATTER_DEPTH {
                    return false;
                }
            }
            ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
}

/// Parse a flat legacy skill file (`<name>.md`) into a Skill.
fn parse_flat_skill(path: &Path, name: &str) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&content);
    let skill_name = normalize_name(frontmatter.get("name").map(String::as_str).unwrap_or(name));
    if skill_name.is_empty() {
        return None;
    }
    Some(Skill {
        id: format!("file-{skill_name}"),
        name: skill_name,
        description: frontmatter.get("description").cloned().unwrap_or_default(),
        content: body,
        model: frontmatter.get("model").cloned(),
        allowed_tools: split_list(frontmatter.get("allowed_tools")),
        permission_mode: frontmatter.get("permission_mode").cloned(),
        paths: split_list(frontmatter.get("paths")),
        work_modes: split_list(frontmatter.get("work_mode")),
        when_to_use: split_list(frontmatter.get("when_to_use")),
        source: SkillSource::File,
        file_path: Some(path.to_string_lossy().to_string()),
        enabled: true,
    })
}

/// Parse a directory-layout SKILL.md (`<dir>/SKILL.md`) into a Skill.
fn parse_skill_dir_skill(skill_md: &Path, dir_name: &str) -> Option<Skill> {
    let content = std::fs::read_to_string(skill_md).ok()?;
    let (frontmatter, body) = parse_skill_frontmatter(&content);

    let name = normalize_name(
        frontmatter
            .get("name")
            .map(String::as_str)
            .unwrap_or(dir_name),
    );
    if name.is_empty() {
        return None;
    }
    let description = frontmatter
        .get("description")
        .cloned()
        .map(|d| d.chars().take(MAX_DESCRIPTION_LEN).collect())
        .filter(|d: &String| !d.is_empty())
        .unwrap_or_else(|| derive_description(&body));

    Some(Skill {
        id: format!("file-{name}"),
        name,
        description,
        content: body,
        model: frontmatter.get("model").cloned(),
        allowed_tools: split_list(frontmatter.get("allowed_tools")),
        permission_mode: frontmatter.get("permission_mode").cloned(),
        paths: split_list(frontmatter.get("paths")),
        work_modes: split_list(frontmatter.get("work_mode")),
        when_to_use: split_list(frontmatter.get("when_to_use")),
        source: SkillSource::File,
        file_path: Some(skill_md.to_string_lossy().to_string()),
        enabled: true,
    })
}

/// Split a comma-separated list value (already joined by the parser).
fn split_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Normalize CRLF → LF for line-oriented parsing. Windows-authored files
/// (Notepad, git autocrlf) carry "\r\n"; the frontmatter parsers match on
/// "\n" fences and would otherwise miss the header entirely.
fn normalize_line_endings(content: &str) -> String {
    if content.contains('\r') {
        content.replace("\r\n", "\n")
    } else {
        content.to_string()
    }
}

/// Parse frontmatter from a markdown file (line-by-line scalar rescue).
fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let content = normalize_line_endings(content);
    let mut frontmatter = HashMap::new();
    let mut body = content.to_string();

    if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm_str = &rest[..end];
            for line in fm_str.lines() {
                if let Some(colon_pos) = line.find(':') {
                    let key = line[..colon_pos].trim().to_string();
                    let value = line[colon_pos + 1..].trim().to_string();
                    frontmatter.insert(key, value);
                }
            }
            body = rest[end + 5..].to_string();
        }
    }

    (frontmatter, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cargo runs tests in parallel in one process — the env-gated eco tests
    /// would otherwise race each other's DEEPDEPCAT_*_SKILLS_ENABLED values.
    static ECO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn make_skill(root: &Path, vendor: &str, name: &str, body: &str) {
        let dir = root.join(vendor).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn loads_project_ecosystem_skills() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        make_skill(
            ws.path(),
            ".claude/skills",
            "deploy",
            "---\nname: deploy\ndescription: Ship to production\n---\nRun the deploy steps.",
        );
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let deploy = skills.iter().find(|s| s.name == "deploy").expect("deploy");
        assert_eq!(deploy.description, "Ship to production");
        assert!(deploy.content.contains("Run the deploy steps"));
    }

    #[test]
    fn vendor_default_skills_are_blacklisted() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        // Claude's own `shell` skill must not leak in from .claude/skills…
        make_skill(
            ws.path(),
            ".claude/skills",
            "shell",
            "---\nname: shell\n---\nx",
        );
        // …but a user's own .deepdepcat/skills/shell is fine.
        make_skill(
            ws.path(),
            ".deepdepcat/skills",
            "shell",
            "---\nname: shell\n---\nx",
        );
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let shell = skills
            .iter()
            .find(|s| s.name == "shell")
            .expect("own shell must load");
        assert!(
            shell
                .file_path
                .as_deref()
                .unwrap_or("")
                .contains(".deepdepcat"),
            "only the own skill survives: {:?}",
            shell.file_path
        );
    }

    #[test]
    fn broken_frontmatter_never_loses_the_skill() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        make_skill(
            ws.path(),
            ".claude/skills",
            "rescue-me",
            "---\nname: rescue-me\ndescription: [unclosed list\n: bad: yaml: here\n---\nBody text",
        );
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let skill = skills
            .iter()
            .find(|s| s.name == "rescue-me")
            .expect("must load");
        assert!(skill.content.contains("Body text"));
    }

    #[test]
    fn own_source_wins_name_collisions() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        make_skill(
            ws.path(),
            ".deepdepcat/skills",
            "dup",
            "---\nname: dup\n---\nown version",
        );
        make_skill(
            ws.path(),
            ".claude/skills",
            "dup",
            "---\nname: dup\n---\neco version",
        );
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let dups: Vec<&Skill> = skills.iter().filter(|s| s.name == "dup").collect();
        assert_eq!(dups.len(), 1, "first-seen-wins dedup");
        assert!(dups[0].content.contains("own version"));
    }

    #[test]
    fn description_derived_from_body_when_missing() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        make_skill(
            ws.path(),
            ".claude/skills",
            "tidy",
            "---\nname: tidy\n---\n# Tidy notes\nOrganizes your workspace.",
        );
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let tidy = skills.iter().find(|s| s.name == "tidy").unwrap();
        assert!(tidy.description.contains("Tidy notes"));
    }

    #[test]
    fn legacy_flat_files_still_load() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(skills_dir.join("flat.md"), "---\nname: flat\n---\nbody").unwrap();
        let loader = SkillLoader::new(dir.path());
        let skills = loader.load_all().unwrap();
        assert!(skills.iter().any(|s| s.name == "flat"));
    }

    #[test]
    fn name_slug_normalization() {
        assert_eq!(normalize_name("My_Skill.v2"), "my-skill-v2");
        assert_eq!(normalize_name("  spaced  name "), "spaced-name");
        assert_eq!(normalize_name(&"a".repeat(100)).len(), MAX_NAME_LEN);
    }

    #[test]
    fn skill_id_traversal_rejected() {
        assert!(validate_skill_id("file-my-skill"));
        assert!(validate_skill_id("code-review"));
        assert!(!validate_skill_id(""), "empty id rejected");
        assert!(!validate_skill_id(".."), "dotdot rejected");
        assert!(!validate_skill_id("."), "dot rejected");
        assert!(!validate_skill_id("../../x"), "path separators rejected");
        assert!(!validate_skill_id("a/b"), "slash rejected");
        assert!(!validate_skill_id(r"..\x"), "backslash rejected");
        assert!(!validate_skill_id("a b"), "space rejected");
    }

    #[test]
    fn frontmatter_nesting_guard_limits_depth() {
        // Flat sequences and maps are fine…
        assert!(frontmatter_within_limits(
            "name: x\nallowed_tools: [a, b, c]"
        ));
        assert!(frontmatter_within_limits("name: x\npayload: {a: 1, b: 2}"));
        // …deeply nested YAML is rejected before it reaches the parser.
        let hostile = format!("a: {}", "[".repeat(64));
        assert!(!frontmatter_within_limits(&hostile));
        let balanced = format!("a: {}x{}", "[".repeat(17), "]".repeat(17));
        assert!(!frontmatter_within_limits(&balanced));
        // Unbalanced closing brackets never trip the depth guard.
        assert!(frontmatter_within_limits("a: [1, 2]]]]"));
    }

    #[test]
    fn crlf_skill_md_parses_frontmatter() {
        // A Windows-authored SKILL.md (Notepad / git autocrlf) uses CRLF —
        // the frontmatter fence is "---\r\n" and must still be detected.
        let content = "---\r\nname: my-skill\r\ndescription: Deploy helper\r\nwhen_to_use: [deploy]\r\n---\r\nBody text\r\n";
        let (fm, body) = parse_skill_frontmatter(content);
        assert_eq!(fm.get("name").map(String::as_str), Some("my-skill"));
        assert_eq!(
            fm.get("description").map(String::as_str),
            Some("Deploy helper")
        );
        assert!(body.contains("Body text"), "body preserved: {body}");
    }

    #[test]
    fn crlf_flat_skill_parses_frontmatter() {
        // The legacy flat parser has the same CRLF requirement.
        let content = "---\r\nname: flat-skill\r\n---\r\nbody\r\n";
        let (fm, body) = parse_frontmatter(content);
        assert_eq!(fm.get("name").map(String::as_str), Some("flat-skill"));
        assert!(body.contains("body"));
    }

    #[test]
    fn frontmatter_fence_without_trailing_newline_does_not_panic() {
        // A SKILL.md whose closing `---` is the last line (no trailing
        // newline — common from editors/tools that don't append one)
        // previously made `rest[end + 5..]` index one byte past the end.
        let content = "---\nname: x\n---";
        let (fm, body) = parse_skill_frontmatter(content);
        assert_eq!(fm.get("name").map(String::as_str), Some("x"));
        assert_eq!(body, "");
    }

    #[test]
    fn deeply_nested_frontmatter_falls_back_to_scalar_rescue() {
        let brackets = format!("{}x{}", "[".repeat(64), "]".repeat(64));
        let content = format!("---\nname: x\npayload: {brackets}\n---\nbody");
        let (fm, body) = parse_skill_frontmatter(&content);
        assert_eq!(body, "body");
        assert_eq!(fm.get("name").map(String::as_str), Some("x"));
        // The YAML tier was skipped; the value stays the raw line text
        // instead of being parsed into a hostile nested structure.
        assert_eq!(
            fm.get("payload").map(String::as_str),
            Some(brackets.as_str())
        );
    }

    #[test]
    fn eco_gate_env_disables_vendor() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        make_skill(ws.path(), ".claude/skills", "x", "---\nname: x\n---\nb");
        std::env::set_var("DEEPDEPCAT_CLAUDE_SKILLS_ENABLED", "0");
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        std::env::remove_var("DEEPDEPCAT_CLAUDE_SKILLS_ENABLED");
        assert!(!skills.iter().any(|s| s.name == "x"), "claude gate off");
    }

    #[test]
    fn config_gate_disables_ecosystem_source() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        make_skill(ws.path(), ".claude/skills", "c1", "---\nname: c1\n---\nb");
        make_skill(
            ws.path(),
            ".deepdepcat/skills",
            "own",
            "---\nname: own\n---\nb",
        );
        let loader = SkillLoader::new(ws.path())
            .with_workspace(Some(ws.path().to_path_buf()))
            .with_compat(false);
        let skills = loader.load_all().unwrap();
        assert!(
            !skills.iter().any(|s| s.name == "c1"),
            "claude gate off via config"
        );
        assert!(
            skills.iter().any(|s| s.name == "own"),
            "own sources unaffected"
        );
    }

    #[test]
    fn plugin_skills_load_with_lowest_priority() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        // Plugin layout: <ws>/.claude/plugins/my-plugin/{plugin.json, skills/<name>/SKILL.md}
        let plugin = ws.path().join(".claude").join("plugins").join("my-plugin");
        std::fs::create_dir_all(plugin.join("skills").join("fetch-news")).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            r#"{"name":"my-plugin","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("skills").join("fetch-news").join("SKILL.md"),
            "---\nname: fetch-news\ndescription: Pull headlines\n---\nFetch.",
        )
        .unwrap();
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let skill = skills
            .iter()
            .find(|s| s.name == "fetch-news")
            .expect("plugin skill");
        assert_eq!(skill.source, SkillSource::Plugin);
        assert!(skill.description.contains("Pull headlines"));
    }

    #[test]
    fn bare_skill_md_plugin_synthesizes_manifest() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        // A plugin directory holding a bare SKILL.md (no plugin.json) is
        // discovered by convention and its dir name becomes the skill name.
        let plugin = ws.path().join(".claude").join("plugins").join("repo-tidy");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("SKILL.md"),
            "---\ndescription: Tidy the repo\n---\nRun tidy.",
        )
        .unwrap();
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        let skill = skills
            .iter()
            .find(|s| s.name == "repo-tidy")
            .expect("bare plugin skill");
        assert_eq!(skill.source, SkillSource::Plugin);
        assert_eq!(skill.description, "Tidy the repo");
    }

    #[test]
    fn non_plugin_dirs_are_ignored() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ws = tempfile::tempdir().unwrap();
        // A directory that is neither a plugin nor a skill (no SKILL.md)
        // must not be treated as a plugin root entry.
        let decoy = ws
            .path()
            .join(".claude")
            .join("plugins")
            .join("not-a-plugin");
        std::fs::create_dir_all(decoy.join("scripts")).unwrap();
        std::fs::write(decoy.join("scripts").join("run.sh"), "echo hi").unwrap();
        let loader = SkillLoader::new(ws.path()).with_workspace(Some(ws.path().to_path_buf()));
        let skills = loader.load_all().unwrap();
        assert!(skills.is_empty() || !skills.iter().any(|s| s.name == "not-a-plugin"));
    }

    #[test]
    fn mode_organized_user_skills_inherit_mode_from_directory() {
        let _g = ECO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        // DEEPDEPCAT_HOME IS the `~/.deepdepcat` dir, so the mode dirs live
        // directly under it: <HOME>/depwork/skills/<name>/SKILL.md.
        let depwork_skill = home
            .path()
            .join("depwork")
            .join("skills")
            .join("depwork-skill")
            .join("SKILL.md");
        std::fs::create_dir_all(depwork_skill.parent().unwrap()).unwrap();
        std::fs::write(
            &depwork_skill,
            "---\nname: depwork-skill\ndescription: d\n---\nbody",
        )
        .unwrap();
        let code_skill = home
            .path()
            .join("code")
            .join("skills")
            .join("code-skill")
            .join("SKILL.md");
        std::fs::create_dir_all(code_skill.parent().unwrap()).unwrap();
        std::fs::write(&code_skill, "---\nname: code-skill\ndescription: c\n---\nbody").unwrap();

        std::env::set_var("DEEPDEPCAT_HOME", home.path());
        let loader = SkillLoader::new(home.path());
        let skills = loader.load_all().unwrap();
        std::env::remove_var("DEEPDEPCAT_HOME");

        let depwork = skills
            .iter()
            .find(|s| s.name == "depwork-skill")
            .expect("depwork-mode skill must load");
        assert_eq!(depwork.work_modes, vec!["depwork"]);
        let code = skills
            .iter()
            .find(|s| s.name == "code-skill")
            .expect("code-mode skill must load");
        assert_eq!(code.work_modes, vec!["code"]);
    }
}
