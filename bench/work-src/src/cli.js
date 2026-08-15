/** Tiny CLI with unhelpful error messages. */
export function runCli(args) {
  const command = args[0];
  if (command === "start") {
    const port = Number(args[1]);
    if (!Number.isInteger(port) || port <= 0) {
      throw new Error("Invalid arguments");
    }
    return `starting on ${port}`;
  }
  if (command === "stop") {
    return "stopped";
  }
  throw new Error("Invalid arguments");
}
