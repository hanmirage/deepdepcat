/**
 * Skill, Hook, Command, Subagent types.
 *
 * Mirrors Rust structs from the backend services.
 */

/** A skill that can be invoked via $skill-name in chat. */
export interface Skill {
  id: string;
  name: string;
  description: string;
  /** Skill content (system prompt or instruction set). */
  content?: string;
  /** File path or bundled indicator. Mirrors Rust SkillSource (bundled|file). */
  source: "bundled" | "file";
  enabled: boolean;
}

/** A hook that runs before/after tool calls. */
export interface Hook {
  id: string;
  name: string;
  /** When the hook fires: before_tool, after_tool, etc. */
  event: string;
  /** Command to execute. */
  command: string;
  enabled: boolean;
}

/** A custom slash command. */
export interface Command {
  id: string;
  name: string;
  description: string;
  /** The template text that gets inserted. */
  template: string;
}

/** A subagent configuration. */
export interface Subagent {
  id: string;
  name: string;
  description: string;
  /** Model used by this subagent. */
  model: string;
  enabled: boolean;
}
