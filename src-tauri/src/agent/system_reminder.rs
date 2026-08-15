//! System reminder injection — periodically inserts contextual reminders
//! into the conversation to guide the agent's behavior.
//!
//! Reminders are injected as system messages appended to the conversation
//! before the next LLM call. They include:
//! - Turn count and elapsed time
//! - Uncommitted changes warning
//! - TODO nudge (encouraging `todo_write` usage)
//! - Cost/token budget warnings

/// Configuration for the reminder system.
#[derive(Debug, Clone)]
pub struct ReminderConfig {
    /// Whether system reminders are enabled at all.
    pub enabled: bool,
    /// Number of turns between reminder injections.
    pub turns_between_reminders: u32,
    /// Minimum turns before the first reminder.
    pub min_turns_before_first: u32,
    /// Whether to nudge the agent to use `todo_write`.
    pub todo_nudge_enabled: bool,
    /// Turns without `todo_write` before nudging.
    pub todo_nudge_threshold: u32,
    /// Whether to warn about uncommitted changes.
    pub uncommitted_warning_enabled: bool,
    /// Threshold (in turns) after which to warn about uncommitted changes.
    pub uncommitted_warning_threshold: u32,
}

impl Default for ReminderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            turns_between_reminders: 5,
            min_turns_before_first: 3,
            todo_nudge_enabled: true,
            todo_nudge_threshold: 3,
            uncommitted_warning_enabled: true,
            uncommitted_warning_threshold: 10,
        }
    }
}

/// State tracked for reminder injection.
#[derive(Debug, Clone, Default)]
pub struct ReminderState {
    /// Total turns since the last reminder was injected.
    turns_since_last_reminder: u32,
    /// Total turns since `todo_write` was last called.
    turns_since_todo_write: u32,
    /// Whether the periodic TODO nudge has already been sent this run.
    ///
    /// The stop-time TodoGate (run.rs) is the disciplined backup: the
    /// periodic reminder repeating the SAME Rule-3 nudge every cycle on top
    /// of it is noise. One periodic nudge + the gate's escalating nudges is
    /// the intended signal budget.
    todo_nudge_periodic_sent: bool,
    /// Whether a reminder is pending injection.
    reminder_pending: bool,
    /// The pending reminder text (if any).
    pending_text: Option<String>,
}

impl ReminderState {
    /// Create new reminder state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Called at the start of each agent turn.
    pub fn on_turn_start(&mut self) {
        self.turns_since_last_reminder += 1;
        self.turns_since_todo_write += 1;
    }

    /// Called when `todo_write` is used by the agent.
    pub fn on_todo_write(&mut self) {
        self.turns_since_todo_write = 0;
    }

    /// Mark the periodic TODO nudge as sent (the stop-time TodoGate has
    /// covered it — further periodic repetition would be noise).
    pub fn mark_todo_nudge_sent(&mut self) {
        self.todo_nudge_periodic_sent = true;
    }

    /// Whether the TodoGate should fire — the agent hasn't used `todo_write`
    /// recently and is about to end the turn.
    pub fn should_fire_todo_gate(&self, config: &ReminderConfig) -> bool {
        config.todo_nudge_enabled && self.turns_since_todo_write >= config.todo_nudge_threshold
    }

    /// Check if a reminder should be injected this turn.
    pub fn should_inject(&self, config: &ReminderConfig, current_turn: u32) -> bool {
        if !config.enabled {
            return false;
        }
        if current_turn < config.min_turns_before_first {
            return false;
        }
        if self.turns_since_last_reminder < config.turns_between_reminders {
            return false;
        }
        true
    }

    /// Build the reminder text to inject.
    pub fn build_reminder(
        &mut self,
        config: &ReminderConfig,
        current_turn: u32,
        elapsed_secs: u64,
        token_usage: u64,
    ) -> Option<String> {
        if !config.enabled {
            return None;
        }

        let mut parts = Vec::new();
        parts.push(format!(
            "[System Reminder] You have been working for {} turn(s) ({}s elapsed, ~{} tokens used).",
            current_turn,
            elapsed_secs,
            token_usage
        ));

        // One-shot TODO nudge: the FIRST cycle at/over the threshold carries
        // it; later cycles (and the stop-time TodoGate) already covered the
        // Rule-3 message — repeating it every 5 turns stacks identical
        // guidance on top of the gate.
        if config.todo_nudge_enabled
            && !self.todo_nudge_periodic_sent
            && self.turns_since_todo_write >= config.todo_nudge_threshold
        {
            self.todo_nudge_periodic_sent = true;
            parts.push(format!(
                "Per <task_completion_discipline> TASK RULE 3, you haven't updated your \
                TODO list in {} turns. If you are in the middle of genuinely \
                multi-step work (three or more distinct actions), use `todo_write` \
                to track progress — but a simple task gets no todo list.",
                self.turns_since_todo_write
            ));
        }

        if config.uncommitted_warning_enabled
            && current_turn >= config.uncommitted_warning_threshold
        {
            parts.push(
                "You have been working for a while. \
                Consider committing your changes if appropriate."
                    .to_string(),
            );
        }

        Some(parts.join(" "))
    }

    /// Mark a reminder as pending injection.
    pub fn set_pending(&mut self, text: String) {
        self.reminder_pending = true;
        self.pending_text = Some(text);
    }

    /// Take the pending reminder (if any) and reset the counter.
    pub fn take_pending(&mut self) -> Option<String> {
        if self.reminder_pending {
            self.reminder_pending = false;
            self.turns_since_last_reminder = 0;
            self.pending_text.take()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_injects_after_threshold() {
        let config = ReminderConfig::default();
        let mut state = ReminderState::new();

        for _ in 0..config.min_turns_before_first {
            state.on_turn_start();
        }
        // Just below threshold
        assert!(!state.should_inject(&config, config.min_turns_before_first));

        for _ in 0..config.turns_between_reminders {
            state.on_turn_start();
        }
        // Now should inject
        assert!(state.should_inject(
            &config,
            config.min_turns_before_first + config.turns_between_reminders
        ));
    }

    #[test]
    fn disabled_config_never_injects() {
        let config = ReminderConfig {
            enabled: false,
            ..Default::default()
        };
        let mut state = ReminderState::new();

        state.on_turn_start();
        state.on_turn_start();
        state.on_turn_start();

        assert!(!state.should_inject(&config, 100));
    }

    #[test]
    fn todo_write_resets_counter() {
        let mut state = ReminderState::new();
        state.on_turn_start();
        state.on_turn_start();
        state.on_turn_start();
        assert_eq!(state.turns_since_todo_write, 3);

        state.on_todo_write();
        assert_eq!(state.turns_since_todo_write, 0);
    }

    #[test]
    fn periodic_todo_nudge_fires_once() {
        let config = ReminderConfig::default();
        let mut state = ReminderState::new();
        for _ in 0..config.todo_nudge_threshold {
            state.on_turn_start();
        }
        let first = state
            .build_reminder(&config, 5, 10, 100)
            .expect("first reminder carries the TODO nudge");
        assert!(
            first.contains("TODO list"),
            "first periodic reminder must nudge todo_write: {first}"
        );

        // A later reminder cycle must NOT repeat the same nudge — the
        // TodoGate covers the stop-time discipline.
        for _ in 0..config.turns_between_reminders {
            state.on_turn_start();
        }
        let second = state
            .build_reminder(&config, 10, 20, 200)
            .expect("later reminder cycles still fire");
        assert!(
            !second.contains("TODO list"),
            "periodic TODO nudge must fire exactly once: {second}"
        );
    }

    #[test]
    fn todo_gate_mark_suppresses_periodic_nudge() {
        let config = ReminderConfig::default();
        let mut state = ReminderState::new();
        state.mark_todo_nudge_sent();
        for _ in 0..config.todo_nudge_threshold {
            state.on_turn_start();
        }
        let reminder = state
            .build_reminder(&config, 5, 10, 100)
            .expect("reminder still fires");
        assert!(
            !reminder.contains("TODO list"),
            "TodoGate coverage must suppress the periodic nudge: {reminder}"
        );
    }
}
