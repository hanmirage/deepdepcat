export const crashDialog = {
  title: "Sorry, DeepDepCat ran into an unexpected crash",
  privacy:
    "DeepDepCat deeply respects your privacy. You choose whether to help us find and fix this — nothing is sent automatically.",
  optionErrorOnly: "Send error code only",
  optionErrorOnlyDesc:
    "Send the crash info (error message, system environment) to help us diagnose. Does not include your conversation.",
  optionWithConversation: "Attach JSON conversation file",
  optionWithConversationDesc:
    "Also attach this conversation (including tool calls) so we can fully reproduce the issue. Sent only if you choose this.",
  notNow: "Not now",
  send: "Send report",
  sending: "Sending…",
  sent: "Sent — thank you for your feedback!",
  sendError: "Send failed: ",
  noSessionToShare: "No conversation available to share — sent the error code only",
  restored: "Restored your last session.",
  readFailed: "Failed to read the crash report",
};
