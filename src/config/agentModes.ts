/**
 * Execution-strategy options — the agent's "how to organize work" modes.
 *
 * Surfaced as slash commands (`/standard`, `/reflexion`, …) in the input bar.
 * Labels/descriptions are i18n keys under the `chat` namespace; `id` is the
 * backend `AgentMode` wire value and the slash-command slug.
 */

import {
  Bot,
  ClipboardList,
  RefreshCw,
  ShieldCheck,
  Target,
  Workflow,
} from "lucide-react";
import type { AgentMode } from "@/types";

export interface AgentModeOption {
  id: AgentMode;
  label: string;
  description: string;
  icon: typeof Workflow;
}

export const AGENT_MODE_OPTIONS: AgentModeOption[] = [
  {
    id: "standard",
    label: "chat.agentStandard",
    description: "chat.agentStandardDesc",
    icon: Workflow,
  },
  {
    id: "plan_execute",
    label: "chat.agentPlanExecute",
    description: "chat.agentPlanExecuteDesc",
    icon: ClipboardList,
  },
  {
    id: "reflexion",
    label: "chat.agentReflexion",
    description: "chat.agentReflexionDesc",
    icon: RefreshCw,
  },
  {
    id: "coordinator",
    label: "chat.agentCoordinator",
    description: "chat.agentCoordinatorDesc",
    icon: Bot,
  },
  {
    id: "evaluator_qa",
    label: "chat.agentEvaluatorQa",
    description: "chat.agentEvaluatorQaDesc",
    icon: ShieldCheck,
  },
  {
    id: "goal",
    label: "chat.agentGoal",
    description: "chat.agentGoalDesc",
    icon: Target,
  },
];
