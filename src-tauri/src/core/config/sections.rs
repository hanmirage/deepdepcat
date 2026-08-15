use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Parse the `[tools] sandbox_profile` config string into a sandbox profile.
/// Unknown or empty values fall back to `Workspace` (process-tree isolation,
/// the historical default). Never fails — a typo must not crash startup.
pub fn parse_sandbox_profile(raw: &str) -> crate::sandbox::executor::SandboxProfile {
    match raw.trim().to_ascii_lowercase().as_str() {
        "strict" => crate::sandbox::executor::SandboxProfile::Strict,
        "read_only" | "readonly" => crate::sandbox::executor::SandboxProfile::ReadOnly,
        "off" | "disabled" | "none" => crate::sandbox::executor::SandboxProfile::Off,
        _ => crate::sandbox::executor::SandboxProfile::Workspace,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSection {
    /// The default workspace directory.
    pub workspace: Option<PathBuf>,
    /// Whether to show the non-git warning at session start.
    pub non_git_warning: bool,
    /// The default model for new sessions.
    pub default_model: String,
    /// The default provider for new sessions.
    pub default_provider: String,
    /// ACP (Agent Client Protocol) server — lets external clients (IDEs,
    /// other agents) drive this app as a remote agent over localhost.
    pub acp_enabled: bool,
    /// Port the ACP server binds on (loopback only).
    pub acp_port: u16,
    /// A2A (Agent2Agent) inbound server — lets OTHER agents orchestrate
    /// this app as an agent (AgentCard + tasks/send/get/cancel).
    pub a2a_enabled: bool,
    /// Port the A2A server binds on (loopback only).
    pub a2a_port: u16,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            workspace: None,
            non_git_warning: true,
            default_model: "deepseek-v4-pro".to_string(),
            default_provider: "deepseek".to_string(),
            acp_enabled: false,
            acp_port: 31524,
            a2a_enabled: false,
            a2a_port: 31525,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSection {
    /// Request timeout in seconds.
    pub request_timeout_secs: u64,
    /// Maximum number of retries.
    pub max_retries: u32,
    /// Base delay for exponential backoff (milliseconds).
    pub retry_base_delay_ms: u64,
    /// Maximum retry delay cap (milliseconds).
    pub retry_max_delay_ms: u64,
    /// Whether to enable prompt caching.
    pub prompt_caching_enabled: bool,
    /// Idle timeout for streaming inference (seconds).
    pub inference_idle_timeout_secs: u64,
    /// Fallback model to use when the primary model fails after all retries.
    /// When set, the request is retried once with this model.
    pub fallback_model: Option<String>,
    /// Per-provider API keys and base URLs.
    pub providers: Vec<ProviderConfig>,
}

impl Default for LlmSection {
    fn default() -> Self {
        Self {
            request_timeout_secs: 120,
            max_retries: 3,
            retry_base_delay_ms: 500,
            retry_max_delay_ms: 32_000,
            prompt_caching_enabled: true,
            inference_idle_timeout_secs: 180,
            fallback_model: None,
            providers: vec![
                ProviderConfig {
                    name: "deepseek".to_string(),
                    api_key_env: "DEEPSEEK_API_KEY".to_string(),
                    api_key: None,
                    // DeepSeek's OpenAI-compatible endpoint is
                    // https://api.deepseek.com/chat/completions — /v1 is a
                    // legacy-compatible alias, not the canonical shape.
                    base_url: "https://api.deepseek.com".to_string(),
                    enabled: true,
                    protocol: None,
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key_env: "OPENAI_API_KEY".to_string(),
                    api_key: None,
                    base_url: "https://api.openai.com/v1".to_string(),
                    enabled: true,
                    protocol: None,
                },
                ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key_env: "ANTHROPIC_API_KEY".to_string(),
                    api_key: None,
                    base_url: "https://api.anthropic.com".to_string(),
                    enabled: true,
                    protocol: None,
                },
                ProviderConfig {
                    name: "grok".to_string(),
                    api_key_env: "XAI_API_KEY".to_string(),
                    api_key: None,
                    base_url: "https://api.x.ai/v1".to_string(),
                    enabled: true,
                    protocol: None,
                },
                ProviderConfig {
                    name: "ollama".to_string(),
                    api_key_env: "".to_string(),
                    api_key: None,
                    base_url: "http://localhost:11434/v1".to_string(),
                    enabled: true,
                    protocol: None,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub name: String,
    /// Environment variable name to read the API key from.
    pub api_key_env: String,
    /// Direct API key (if set, overrides env var).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub base_url: String,
    pub enabled: bool,
    /// Wire protocol for this provider: `"openai"` (chat completions),
    /// `"anthropic"` (Messages API) or `"responses"` (OpenAI Responses API).
    /// `None` auto-detects: provider named "anthropic" → Anthropic,
    /// everything else → OpenAI chat completions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            api_key_env: String::new(),
            api_key: None,
            base_url: String::new(),
            enabled: true,
            protocol: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSection {
    /// Maximum turns per agent loop.
    pub max_turns: u32,
    /// Auto-compact threshold (percentage of context window, 0-100).
    pub auto_compact_threshold_percent: u8,
    /// Whether two-pass compaction is enabled.
    pub two_pass_compaction: bool,
    /// Model to use for conversation compaction (usually a fast/cheap model).
    pub compaction_model: String,
    /// Optional per-role model matrix (P3-6): subagents of a given type use
    /// a dedicated model instead of inheriting the parent's. `None` = inherit.
    /// Plan agents are usually fast/cheap (they only design), explore agents
    /// cheap too (read-only search), verify agents strong (they judge work).
    #[serde(default)]
    pub plan_model: Option<String>,
    #[serde(default)]
    pub explore_model: Option<String>,
    #[serde(default)]
    pub verify_model: Option<String>,
    /// Maximum concurrent tool calls per turn.
    pub max_concurrent_tools: usize,
    /// Whether to show thinking/reasoning blocks.
    pub show_thinking: bool,
    /// Whether multi-agent (subagent) is enabled.
    pub multi_agent_enabled: bool,
    /// Maximum subagent depth.
    pub max_subagent_depth: u32,
    /// Session-level total token limit (0 = unlimited).
    pub session_token_limit: u64,
    /// Session-level total cost limit in USD (0.0 = unlimited).
    pub session_cost_limit: f64,
    /// Wall-clock timeout for one loop invocation in seconds
    /// (None = unlimited).
    #[serde(default)]
    pub run_timeout_secs: Option<u64>,
    /// Optional per-turn OUTPUT token cap (0/None = unlimited).
    ///
    /// When set, each LLM request carries this `max_tokens` and the loop
    /// RESPECTS it — truncation recovery does not escalate past the user's
    /// explicit cap. Leave unset to keep the default escalation behavior.
    #[serde(default)]
    pub turn_output_token_limit: Option<u64>,
    /// DeepSeek optimization master switch — mirrors the frontend settings
    /// toggle. When ON for a DeepSeek session, compaction uses cache-aware
    /// summarization (summary calls reuse the session prefix) and
    /// prune-before-summarize; OFF or non-DeepSeek keeps the plain path.
    #[serde(default = "default_true")]
    pub deepseek_auto_reasoning: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            max_turns: 50,
            auto_compact_threshold_percent: 80,
            two_pass_compaction: false,
            compaction_model: "deepseek-v4-flash".to_string(),
            plan_model: None,
            explore_model: None,
            verify_model: None,
            max_concurrent_tools: 5,
            show_thinking: true,
            multi_agent_enabled: true,
            max_subagent_depth: 3,
            session_token_limit: 0,
            session_cost_limit: 0.0,
            run_timeout_secs: None,
            turn_output_token_limit: None,
            deepseek_auto_reasoning: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsSection {
    /// Maximum output size for tool results (characters).
    pub max_output_chars: usize,
    /// Whether web_fetch is enabled.
    pub web_fetch_enabled: bool,
    /// Whether web_search is enabled.
    pub web_search_enabled: bool,
    /// Whether bash tool is enabled.
    pub bash_enabled: bool,
    /// Whether LSP tools are enabled.
    pub lsp_enabled: bool,
    /// Bash command timeout (seconds).
    pub bash_timeout_secs: u64,
    /// Whether to auto-background bash on timeout.
    pub auto_background_on_timeout: bool,
    /// Behavior version pinned for all tools ("current" | "legacy-0.1.0").
    /// `current` is the default; legacy versions preserve historical output
    /// formats for users upgrading from older releases.
    pub behavior_version: String,
    /// AMap (高德) Open Platform Web Service API key — powers
    /// store_research_geo (POI search + place/around). Free tier, apply at
    /// https://console.amap.com/ . Empty = the tool reports a setup hint.
    pub amap_web_key: String,
    /// Sandbox isolation profile for the bash tool. Values: "workspace"
    /// (default — process-tree isolation via Job Object), "strict" /
    /// "read_only" (additionally strip admin SIDs via a restricted token),
    /// "off" (no isolation). Empty = "workspace".
    pub sandbox_profile: String,
}

impl Default for ToolsSection {
    fn default() -> Self {
        Self {
            max_output_chars: 30_000,
            web_fetch_enabled: true,
            web_search_enabled: true,
            bash_enabled: true,
            lsp_enabled: true,
            bash_timeout_secs: 120,
            auto_background_on_timeout: true,
            behavior_version: "current".to_string(),
            amap_web_key: String::new(),
            sandbox_profile: "workspace".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsSection {
    /// Default permission mode.
    pub mode: String,
    /// Allow rules (tool patterns).
    pub allow: Vec<String>,
    /// Deny rules.
    pub deny: Vec<String>,
    /// Ask rules.
    pub ask: Vec<String>,
    /// Maximum consecutive denials before stopping.
    pub max_consecutive_denials: u32,
    /// Route gray-zone approval asks through an independent reviewer agent
    /// instead of always stopping for a human (Auto-Review). The reviewer
    /// is a swap, never a grant: rule denials and sensitive-file/dangerous
    /// gates still win and never reach it.
    pub auto_review: bool,
    /// Model used for Auto-Review verdicts (empty = default model).
    pub auto_review_model: String,
    /// Bash network command policy: "allow_all" (default) | "block" |
    /// "allowlist". Command-level only — see docs/SANDBOX_BOUNDARIES.md.
    pub network_policy_mode: String,
    /// Allowed domains when `network_policy_mode = "allowlist"`. Apex and
    /// subdomains both match (`example.com` covers `api.example.com`).
    pub network_policy_domains: Vec<String>,
    /// Whether private/local targets (127.0.0.1, 10.x, localhost, …) are
    /// allowed. Default true keeps localhost tooling working.
    pub network_allow_private: bool,
}

impl Default for PermissionsSection {
    fn default() -> Self {
        Self {
            mode: "accept_edits".to_string(),
            allow: vec![
                "Read(*)".to_string(),
                "ListDir(*)".to_string(),
                "Glob(*)".to_string(),
                "Grep(*)".to_string(),
            ],
            deny: vec![
                "Read(~/.ssh/*)".to_string(),
                "Read(~/.env)".to_string(),
                "Read(**/.env)".to_string(),
            ],
            ask: vec![],
            max_consecutive_denials: 5,
            auto_review: false,
            auto_review_model: String::new(),
            network_policy_mode: "allow_all".to_string(),
            network_policy_domains: Vec::new(),
            network_allow_private: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageSection {
    /// Database file path (relative to app data dir if not absolute).
    pub database_path: String,
    /// Whether to enable WAL mode.
    pub wal_mode: bool,
    /// Maximum database size in MB (0 = unlimited).
    pub max_size_mb: u64,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self {
            database_path: "deepdepcat.db".to_string(),
            wal_mode: true,
            max_size_mb: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSection {
    /// MCP server configurations.
    pub servers: Vec<McpServerConfig>,
    /// Maximum MCP output size in bytes.
    pub max_output_bytes: u64,
    /// MCP startup timeout (seconds).
    pub startup_timeout_secs: u64,
}

impl Default for McpSection {
    fn default() -> Self {
        Self {
            servers: vec![],
            max_output_bytes: 20_000,
            startup_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub transport_type: String, // "stdio" | "sse" | "http"
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub url: Option<String>,
    pub enabled: bool,
}

/// Skill system settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SkillsSection {
    /// Scan Claude ecosystem skills/plugins (`~/.claude/skills`,
    /// `<ws>/.claude/skills`, `~/.claude/plugins`, `<ws>/.claude/plugins`).
    /// Overridable via `DEEPDEPCAT_CLAUDE_SKILLS_ENABLED`.
    pub claude_enabled: bool,
}

impl Default for SkillsSection {
    fn default() -> Self {
        Self {
            claude_enabled: true,
        }
    }
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport_type: "stdio".to_string(),
            command: None,
            args: vec![],
            env: std::collections::HashMap::new(),
            url: None,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HooksSection {
    /// Master switch for PROJECT-level hooks (`<workspace>/.deepdepcat/
    /// hooks.toml`). Default OFF: opening a cloned repository must not
    /// execute arbitrary commands until the user explicitly opts in.
    /// User-level hooks (`~/.deepdepcat/hooks.toml`) are unaffected — they
    /// are the user's own configuration.
    pub enable_project_hooks: bool,
    /// Hook definitions keyed by event name.
    pub definitions: Vec<HookDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookDefinition {
    pub event: String,
    #[serde(rename = "type")]
    pub hook_type: String, // "command" | "prompt" | "agent" | "http"
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "if")]
    pub condition: Option<String>,
    pub timeout_ms: Option<u64>,
    pub shell: Option<String>,
}

impl Default for HookDefinition {
    fn default() -> Self {
        Self {
            event: String::new(),
            hook_type: "command".to_string(),
            command: None,
            prompt: None,
            url: None,
            condition: None,
            timeout_ms: Some(30_000),
            shell: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemorySection {
    pub enabled: bool,
    /// Maximum number of memory search results.
    pub search_max_results: u32,
    /// Minimum similarity score for memory search.
    pub search_min_score: f32,
    /// Whether to auto-inject memories into context.
    pub auto_injection_enabled: bool,
    /// Whether the memory watcher is enabled.
    pub watcher_enabled: bool,
    /// Whether the dream (background synthesis) is enabled.
    pub dream_enabled: bool,
    /// Minimum hours between dream synthesis cycles.
    pub dream_min_hours: u64,
    /// Minimum memories accumulated before synthesis is worth it.
    pub dream_min_memories: usize,
    /// Hybrid search weight for BM25 keyword score (0.0–1.0).
    pub search_weight_bm25: f32,
    /// Hybrid search weight for cosine similarity score (0.0–1.0).
    pub search_weight_cosine: f32,
    /// Hybrid search weight for recency/access-frequency score (0.0–1.0).
    pub search_weight_recency: f32,
    /// Half-life (hours) of the recency decay — memories are exponentially
    /// less relevant the longer they go unaccessed.
    pub search_recency_half_life_hours: u64,
    /// Recency temperature — the time-decay component is raised to this
    /// power. > 1 sharpens recency dominance, < 1 flattens it.
    pub search_recency_temperature: f32,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            enabled: true,
            search_max_results: 10,
            search_min_score: 0.5,
            auto_injection_enabled: true,
            watcher_enabled: true,
            dream_enabled: false,
            dream_min_hours: 24,
            dream_min_memories: 3,
            search_weight_bm25: 0.4,
            search_weight_cosine: 0.4,
            search_weight_recency: 0.2,
            search_recency_half_life_hours: 168,
            search_recency_temperature: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSection {
    pub theme: String,
    pub font_size: u32,
    pub show_token_count: bool,
    pub show_cost: bool,
}

impl Default for UiSection {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            font_size: 14,
            show_token_count: true,
            show_cost: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetrySection {
    pub enabled: bool,
    /// "off" | "session-metrics" | "full"
    pub mode: String,
}

impl Default for TelemetrySection {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "off".to_string(),
        }
    }
}

/// Vision model configuration — the multimodal endpoint used to transcribe
/// user-attached images into text for text-only main models (DeepSeek).
///
/// The vision model is a user-supplied, OpenAI-compatible multimodal endpoint
/// (e.g. GLM-4V-Flash, qwen-vl). The config is SELF-CONTAINED — the user fills
/// base_url/api_key/model directly in Settings → Model Providers → Vision
/// model, without needing a separate `llm.providers` entry. This is because the
/// main chat model may be text-only (DeepSeek, GLM text), while the vision
/// model is what lets the agent "see" images.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisionSection {
    /// Master switch for image transcription. When false, attached images
    /// fail with a clear "configure a vision model" error.
    pub enabled: bool,
    /// Base URL of the OpenAI-compatible vision endpoint.
    pub base_url: String,
    /// API key for the vision endpoint (may be empty for key-less local/free
    /// models).
    pub api_key: String,
    /// Model id on that endpoint (e.g. "glm-4v-flash").
    pub model: String,
}
