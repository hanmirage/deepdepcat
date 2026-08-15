/**
 * Chinese translations — split by namespace.
 */
import { common } from "./common";
import { crashDialog } from "./crashDialog";
import { chat } from "./chat";
import { reasoning } from "./reasoning";
import { timeGreeting } from "./timeGreeting";
import { hints } from "./hints";
import { toolCall } from "./toolCall";
import { depwork } from "./depwork";
import { sidebar } from "./sidebar";
import { notifications } from "./notifications";
import { update } from "./update";
import { layout } from "./layout";
import { activity } from "./activity";
import { settings } from "./settings";
import { agentStatus } from "./agentStatus";
import { debug } from "./debug";
import { permission } from "./permission";
import { planApproval } from "./planApproval";
import { rightPanel } from "./rightPanel";
import { depworkContext } from "./depworkContext";
import { customize } from "./customize";
import { depworkTask } from "./depworkTask";
import { settingsGroups } from "./settingsGroups";
import { settingsCategories } from "./settingsCategories";
import { onboarding } from "./onboarding";
import { officeTyping } from "./officeTyping";
import { scheduled } from "./scheduled";
import { subagents } from "./subagents";
import { task } from "./task";
import { preview, takeover } from "./preview";

export const zh = {
  common,
  crashDialog,
  chat,
  reasoning,
  timeGreeting,
  hints,
  toolCall,
  depwork,
  sidebar,
  notifications,
  update,
  layout,
  activity,
  settings,
  agentStatus,
  debug,
  permission,
  planApproval,
  rightPanel,
  depworkContext,
  customize,
  depworkTask,
  settingsGroups,
  settingsCategories,
  onboarding,
  officeTyping,
  scheduled,
  subagents,
  task,
  preview,
  takeover,
};

export type TranslationSchema = typeof zh;
