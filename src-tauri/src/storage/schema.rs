//! Database schema — table definitions and migration SQL.
//!
//! Each migration is a versioned SQL block. Migrations are applied in order
//! and tracked in the `_migrations` table.

/// The list of migrations, in order. Each entry is (version, description, sql).
pub static MIGRATIONS: &[(i64, &str, &str)] = &[
    // ── Migration 1: Initial schema ──────────────────────────────────────────
    (
        1,
        "Initial schema",
        r#"
        -- Sessions table
        CREATE TABLE IF NOT EXISTS sessions (
            id              TEXT PRIMARY KEY,
            title           TEXT NOT NULL DEFAULT 'New Session',
            model           TEXT NOT NULL,
            provider        TEXT NOT NULL,
            status          TEXT NOT NULL DEFAULT 'active',
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL,
            workspace_path  TEXT,
            system_prompt   TEXT NOT NULL DEFAULT '',
            turn_count      INTEGER NOT NULL DEFAULT 0,
            prompt_tokens   INTEGER NOT NULL DEFAULT 0,
            completion_tokens INTEGER NOT NULL DEFAULT 0,
            cached_read_tokens INTEGER,
            reasoning_tokens INTEGER
        );

        -- Messages table (conversation history)
        CREATE TABLE IF NOT EXISTS messages (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id      TEXT NOT NULL,
            role            TEXT NOT NULL,  -- system, user, assistant, tool_result, reasoning
            content         TEXT NOT NULL,
            tool_call_id    TEXT,           -- for tool_result role
            tool_name       TEXT,           -- for tool_result role
            tool_calls      TEXT,           -- JSON array of tool calls (for assistant role)
            is_error        INTEGER NOT NULL DEFAULT 0,
            model           TEXT,           -- for assistant role
            prompt_tokens   INTEGER,        -- for assistant role
            completion_tokens INTEGER,      -- for assistant role
            cached_read_tokens INTEGER,     -- for assistant role
            reasoning_tokens INTEGER,       -- for assistant role
            created_at      TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
        CREATE INDEX IF NOT EXISTS idx_messages_created ON messages(created_at);

        -- Settings table (key-value store)
        CREATE TABLE IF NOT EXISTS settings (
            key             TEXT PRIMARY KEY,
            value           TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        );

        -- Tasks table (cowork / background tasks)
        CREATE TABLE IF NOT EXISTS tasks (
            id              TEXT PRIMARY KEY,
            description     TEXT NOT NULL,
            status          TEXT NOT NULL DEFAULT 'pending',
            context_paths   TEXT,  -- JSON array
            session_id      TEXT,
            created_at      TEXT NOT NULL,
            completed_at    TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE SET NULL
        );

        -- Memory table (FTS5 virtual table for full-text search)
        CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
            content,
            metadata,
            category,
            session_id UNINDEXED,
            created_at UNINDEXED,
            tokenize = 'unicode61'
        );

        -- Memory metadata table (non-FTS columns)
        CREATE TABLE IF NOT EXISTS memory (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            content         TEXT NOT NULL,
            metadata        TEXT NOT NULL DEFAULT '{}',
            category        TEXT NOT NULL DEFAULT 'general',
            session_id      TEXT,
            embedding       BLOB,  -- vector embedding (if available)
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL,
            access_count    INTEGER NOT NULL DEFAULT 0,
            last_accessed   TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE SET NULL
        );

        -- MCP servers table
        CREATE TABLE IF NOT EXISTS mcp_servers (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            transport_type  TEXT NOT NULL DEFAULT 'stdio',
            command         TEXT,
            args            TEXT,  -- JSON array
            env             TEXT,  -- JSON object
            url             TEXT,
            enabled         INTEGER NOT NULL DEFAULT 1,
            status          TEXT NOT NULL DEFAULT 'disconnected',
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        );

        -- Hook definitions table
        CREATE TABLE IF NOT EXISTS hooks (
            id              TEXT PRIMARY KEY,
            event           TEXT NOT NULL,
            hook_type       TEXT NOT NULL DEFAULT 'command',
            command         TEXT,
            prompt          TEXT,
            url             TEXT,
            condition       TEXT,
            timeout_ms      INTEGER,
            shell           TEXT,
            enabled         INTEGER NOT NULL DEFAULT 1,
            created_at      TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_hooks_event ON hooks(event);
        "#,
    ),
    // ── Migration 2: Add conversation_order to messages ──────────────────────
    (
        2,
        "Add conversation_order and parent_message_id to messages",
        r#"
        ALTER TABLE messages ADD COLUMN conversation_order INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE messages ADD COLUMN parent_message_id INTEGER;

        -- Create index for ordered retrieval
        CREATE INDEX IF NOT EXISTS idx_messages_order ON messages(session_id, conversation_order);
        "#,
    ),
    // ── Migration 3: Add cost tracking ────────────────────────────────────────
    (
        3,
        "Add cost tracking to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN total_cost REAL NOT NULL DEFAULT 0.0;

        ALTER TABLE messages ADD COLUMN cost REAL;
        "#,
    ),
    // ── Migration 4: Add decay_factor and embedding_dim to memory ──────────────
    (
        4,
        "Add decay_factor to memory table",
        r#"
        ALTER TABLE memory ADD COLUMN decay_factor REAL NOT NULL DEFAULT 1.0;
        "#,
    ),
    // ── Migration 5: Add reasoning_content to messages ───────────────────────
    (
        5,
        "Add reasoning_content column for assistant messages",
        r#"
        ALTER TABLE messages ADD COLUMN reasoning_content TEXT;
        "#,
    ),
    // ── Migration 6: Add DeepSeek KV Cache columns ────────────────────────────
    // deepseek-native: prompt_cache_hit_tokens / prompt_cache_miss_tokens
    // are read by list_sessions, get_session, and load_messages but were
    // missing from the original schema, causing "Failed to load sessions".
    (
        6,
        "Add KV cache columns to sessions and messages",
        r#"
        ALTER TABLE sessions ADD COLUMN prompt_cache_hit_tokens INTEGER;
        ALTER TABLE sessions ADD COLUMN prompt_cache_miss_tokens INTEGER;

        ALTER TABLE messages ADD COLUMN prompt_cache_hit_tokens INTEGER;
        ALTER TABLE messages ADD COLUMN prompt_cache_miss_tokens INTEGER;
        "#,
    ),
    // ── Migration 7: Add rewind checkpoint tables ─────────────────────────────
    (
        7,
        "Add rewind checkpoint tables",
        r#"
        -- Rewind points per session per turn.
        CREATE TABLE IF NOT EXISTS rewind_points (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id      TEXT NOT NULL,
            turn_index      INTEGER NOT NULL,
            created_at      TEXT NOT NULL,
            snapshots_json  TEXT NOT NULL DEFAULT '{}',
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_rewind_session ON rewind_points(session_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_rewind_session_turn ON rewind_points(session_id, turn_index);
        "#,
    ),
    // ── Migration 8: Global usage aggregate (single accumulating row) ─────────
    (
        8,
        "Global usage aggregate table",
        r#"
        -- Cumulative usage across ALL sessions, all time. A singleton row
        -- (id = 1) is incremented on every LLM call / tool result so the
        -- settings usage page never loses data — even across app restarts.
        CREATE TABLE IF NOT EXISTS usage_aggregate (
            id                  INTEGER PRIMARY KEY CHECK (id = 1),
            prompt_tokens       INTEGER NOT NULL DEFAULT 0,
            completion_tokens   INTEGER NOT NULL DEFAULT 0,
            cached_read_tokens  INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens    INTEGER NOT NULL DEFAULT 0,
            cache_hit_tokens    INTEGER NOT NULL DEFAULT 0,
            cache_miss_tokens   INTEGER NOT NULL DEFAULT 0,
            tool_calls          INTEGER NOT NULL DEFAULT 0,
            tool_result_tokens  INTEGER NOT NULL DEFAULT 0,
            turns               INTEGER NOT NULL DEFAULT 0,
            updated_at          TEXT NOT NULL DEFAULT ''
        );
        "#,
    ),
    // ── Migration 9: Rebuild memory_fts for CJK-friendly indexing ─────────────
    // The default unicode61 tokenizer splits on whitespace, so a Chinese
    // sentence becomes ONE token ("帮我记住技术栈" is not searchable by
    // "技术"). Indexed text is now spaced into 2-char CJK windows before
    // insertion (see `cjk_bigram_spacing` in memory/store.rs) — "技术栈"
    // indexes as "技术 术栈", so a search for either the full word or a
    // partial term hits. Latin text is untouched (phrases stay exact via
    // the double-quoted query terms).
    (
        9,
        "Rebuild memory_fts with CJK bigram spacing",
        r#"
        DROP TABLE IF EXISTS memory_fts;
        CREATE VIRTUAL TABLE memory_fts USING fts5(
            content,
            metadata,
            category,
            session_id UNINDEXED,
            created_at UNINDEXED,
            tokenize = 'unicode61'
        );
        INSERT INTO memory_fts (rowid, content, metadata, category, session_id, created_at)
            SELECT id, content, metadata, category, session_id, created_at FROM memory;
        "#,
    ),
    // ── Migration 10: Work mode per session ──────────────────────────────────
    // code/depwork — lets the frontend restore a session into the correct
    // product mode instead of always falling back to Code. Default 'code'
    // keeps pre-existing sessions on the current behavior.
    (
        10,
        "Add work_mode to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN work_mode TEXT NOT NULL DEFAULT 'code';
        "#,
    ),
    // ── Migration 11: Rewind after-snapshots ────────────────────────────────
    // rewind_points originally stored ONLY before-snapshots (snapshots_json),
    // so a restored point had no after state — external-modification conflict
    // detection in `FileStateTracker::rewind_to` was silently disabled for
    // persisted points. Add a parallel column for after-snapshots.
    (
        11,
        "Add after_snapshots_json to rewind_points",
        r#"
        ALTER TABLE rewind_points ADD COLUMN after_snapshots_json TEXT NOT NULL DEFAULT '{}';
        "#,
    ),
    // ── Migration 12: Session goal persistence ───────────────────────────────
    // The session goal (task intent, update_goal tool) must survive app
    // restarts — it is re-injected as <current-goal> every turn, so a goal
    // that died with the process loses the task context. NULL = no goal.
    (
        12,
        "Add goal to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN goal TEXT;
        "#,
    ),
    // ── Migration 13: Session todo list persistence ──────────────────────────
    // The todo list (todo_write tool) must survive restarts so the frontend
    // can restore the task-progress panel. JSON array of TodoItem; NULL/'' =
    // no list.
    (
        13,
        "Add todos to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN todos TEXT;
        "#,
    ),
    // ── Migration 14: Approved plan steps persistence ────────────────────────
    // The structured approved-plan steps (plan gate, `take_active_plan_steps`)
    // were pure memory — an app restart lost the checklist, so a session
    // resumed mid-plan could not nudge the model through the approved steps.
    // JSON array of PlanStep; NULL = no approved plan.
    (
        14,
        "Add plan_steps to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN plan_steps TEXT;
        "#,
    ),
    // ── Migration 15: Real context window per session ───────────────────────
    // The provider's /models metadata (fetched by the frontend) is passed at
    // session creation so compaction/token budgeting use the real window
    // instead of the built-in catalog default. 0 = unknown → catalog fallback.
    (
        15,
        "Add context_window to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN context_window INTEGER NOT NULL DEFAULT 0;
        "#,
    ),
    // ── Migration 16: Per-session permission mode ─────────────────────────
    // Each conversation owns its permission mode (Code vs Depwork have
    // completely different sets). '' = inherit the global default; any
    // canonical mode string overrides it for this session across restarts.
    (
        16,
        "Add permission_mode to sessions",
        r#"
        ALTER TABLE sessions ADD COLUMN permission_mode TEXT NOT NULL DEFAULT '';
        "#,
    ),
    // ── Migration 17: Replay-exact agent event log ──────────────────────
    // Append-only audit log of every model call / tool run / permission
    // decision / file edit per session (Meta Muse Code's local event log
    // pattern). `seq` is per-session monotonic so a turn can be replayed in
    // exact order after a crash. Payloads are truncated/summary-shaped —
    // never full command output or secrets. Foreign key cascades so
    // deleting a session removes its events with it.
    (
        17,
        "Agent event log",
        r#"
        CREATE TABLE IF NOT EXISTS agent_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            turn_id     TEXT,
            seq         INTEGER NOT NULL,
            kind        TEXT NOT NULL,
            payload     TEXT NOT NULL DEFAULT '{}',
            created_at  TEXT NOT NULL,
            created_ms  INTEGER NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_agent_events_session ON agent_events(session_id, seq);
        CREATE INDEX IF NOT EXISTS idx_agent_events_turn ON agent_events(turn_id);
        "#,
    ),
    // ── Migration 18: Research items (Depwork 调研资料夹) ─────────────
    // The Depwork research workflow's source folder: every saved source
    // (literature, web page, snapshot) lives here so the agent can cite it
    // with a stable URL + access date. Per-session ownership; deleting a
    // session removes its research items with it.
    (
        18,
        "Research items",
        r#"
        CREATE TABLE IF NOT EXISTS research_items (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            title       TEXT NOT NULL,
            url         TEXT NOT NULL,
            source      TEXT NOT NULL DEFAULT 'web',
            snippet     TEXT NOT NULL DEFAULT '',
            snapshot    TEXT NOT NULL DEFAULT '',
            tags        TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL,
            created_ms  INTEGER NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_research_session ON research_items(session_id, created_ms);
        "#,
    ),
    // ── Migration 19: Scheduled agent tasks ─────────────────────────
    // Persistent scheduled tasks that run a full AGENT session in the
    // background (unlike the shell-command scheduler, which only runs a
    // command). `schedule_kind` is 'interval' (every_secs) or 'daily'
    // (daily_time "HH:MM" in local time). `project_path` selects the
    // working directory; `use_worktree` isolates git work into a linked
    // worktree that stays behind for review. Runs are append-only so the
    // Scheduled inbox keeps a history with session links + summaries.
    (
        19,
        "Scheduled agent tasks and runs",
        r#"
        CREATE TABLE IF NOT EXISTS scheduled_tasks (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            prompt          TEXT NOT NULL,
            schedule_kind   TEXT NOT NULL DEFAULT 'interval',
            every_secs      INTEGER NOT NULL DEFAULT 0,
            daily_time      TEXT NOT NULL DEFAULT '',
            project_path    TEXT NOT NULL DEFAULT '',
            use_worktree    INTEGER NOT NULL DEFAULT 0,
            work_mode       TEXT NOT NULL DEFAULT 'code',
            model           TEXT NOT NULL DEFAULT '',
            active          INTEGER NOT NULL DEFAULT 1,
            last_run_at_ms  INTEGER,
            run_count       INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS scheduled_runs (
            id              TEXT PRIMARY KEY,
            task_id         TEXT NOT NULL,
            session_id      TEXT,
            status          TEXT NOT NULL DEFAULT 'pending',
            started_at      TEXT NOT NULL,
            finished_at     TEXT,
            summary         TEXT NOT NULL DEFAULT '',
            error           TEXT NOT NULL DEFAULT '',
            worktree_path   TEXT NOT NULL DEFAULT '',
            FOREIGN KEY (task_id) REFERENCES scheduled_tasks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_task ON scheduled_runs(task_id, started_at);
        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_status ON scheduled_runs(status);
        "#,
    ),
    // ── Migration 20: A2A task persistence ──────────────────────────
    // A2A tasks used to live only in process memory — an app restart made
    // every in-flight/completed task un-pollable for external agents. The
    // full Task JSON is stored so `tasks/get` survives restarts; sessions
    // are intentionally NOT foreign-keyed (a closed session must not
    // cascade-delete an orchestration record).
    (
        20,
        "A2A task persistence",
        r#"
        CREATE TABLE IF NOT EXISTS a2a_tasks (
            id          TEXT PRIMARY KEY,
            session_id  TEXT,
            task_json   TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_updated ON a2a_tasks(updated_at);
        "#,
    ),
    // ── Migration 21: Persistent scheduled agents ─────────────────────
    // A scheduled task with `persistent = 1` reuses ONE session across fires
    // (its context/goal accumulates — the agent "lives"), instead of a fresh
    // disposable session per run. `persistent_session_id` is written back by
    // the runner on the first fire and reused on subsequent ones.
    (
        21,
        "Persistent scheduled agents",
        r#"
        ALTER TABLE scheduled_tasks ADD COLUMN persistent INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE scheduled_tasks ADD COLUMN persistent_session_id TEXT;
        "#,
    ),
    // ── Migration 22: Session pin + last-message preview ─────────────
    // Sidebar enhancements: `pinned` pins a session to the top of the list
    // (persisted across restarts); `last_message` is a short preview of the
    // final message, refreshed on each persist, shown under the list row.
    (
        22,
        "Session pin and last-message preview",
        r#"
        ALTER TABLE sessions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE sessions ADD COLUMN last_message TEXT NOT NULL DEFAULT '';
        "#,
    ),
];
