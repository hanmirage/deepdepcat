#!/usr/bin/env node
/**
 * DeepDepCat benchmark runner (Code tasks via ACP).
 *
 * Usage:
 *   node bench/run_bench.mjs --base http://127.0.0.1:31524 --out bench/results/run-YYYY-MM-DD
 *   node bench/run_bench.mjs --task fix-format-bug   # run a single task
 *
 * Prereqs: app running with ACP enabled (Settings → 常规 → ACP 服务),
 * DeepSeek key configured, bench/work is a git repo (reset before each task).
 */

import { readFile, writeFile, mkdir } from "node:fs/promises";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const WORK_DIR = path.join(HERE, "work");
const WORK_SRC_DIR = path.join(HERE, "work-src");
const FIXTURES_DIR = path.join(HERE, "fixtures");
const TASKS_FILE = path.join(HERE, "tasks", "code.md");
const TASK_TIMEOUT_MS = 25 * 60 * 1000;

function parseArgs(argv) {
  const args = { base: "http://127.0.0.1:31524", out: null, task: null };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--base") args.base = argv[++i];
    else if (argv[i] === "--out") args.out = argv[++i];
    else if (argv[i] === "--task") args.task = argv[++i];
  }
  return args;
}

/** Parse `## Task N: id` sections from the tasks markdown. */
function parseTasks(md) {
  const tasks = [];
  const re = /^## Task \d+: ([a-z0-9-]+)\n- \*\*任务\*\*：(.+?)\n- \*\*Acceptance\*\*：(.+?)(?=\n## |\n$)/gms;
  for (const match of md.matchAll(re)) {
    tasks.push({ id: match[1], prompt: match[2].trim(), acceptance: match[3].trim() });
  }
  return tasks;
}

/** Extract the LAST evaluator verdict (M2 tier-3 review) from a transcript. */
function extractVerdict(transcript) {
  const text = transcript
    .filter((e) => e.event === "prompt/streaming_update")
    .map((e) => e.payload?.text ?? "")
    .join("\n");
  const matches = [...text.matchAll(/VERDICT:\s*(PASS|FAIL)/gi)];
  if (matches.length === 0) return null;
  return matches[matches.length - 1][1].toUpperCase();
}

async function rpc(base, method, params) {
  const res = await fetch(`${base}/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: crypto.randomUUID(), method, params }),
  });
  const body = await res.json();
  if (body.error) throw new Error(`${method}: ${body.error.message ?? JSON.stringify(body.error)}`);
  return body.result;
}

/** Consume the SSE stream until the prompt reaches a terminal state. */
async function collectPrompt(base, sessionId, promptId, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const res = await fetch(`${base}/events`, { signal: controller.signal });
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const transcript = [];
  let terminal = null;

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let sep;
      while ((sep = buffer.indexOf("\n\n")) >= 0) {
        const rawEvent = buffer.slice(0, sep);
        buffer = buffer.slice(sep + 2);
        let event = "message";
        let data = "";
        for (const line of rawEvent.split("\n")) {
          if (line.startsWith("event:")) event = line.slice(6).trim();
          else if (line.startsWith("data:")) data += line.slice(5).trim();
        }
        if (!data) continue;
        let payload;
        try {
          payload = JSON.parse(data);
        } catch {
          continue;
        }
        transcript.push({ event, payload });
        if (event === "prompt/update") {
          if (payload.promptId === promptId && (payload.state === "completed" || payload.state === "failed")) {
            terminal = payload;
            break;
          }
        }
      }
      if (terminal) break;
    }
  } finally {
    clearTimeout(timer);
    reader.releaseLock();
  }
  return { terminal, transcript };
}

function resetWork(taskId) {
  // The baseline (work-src) is a GREEN project; each task then overlays
  // its own defect from fixtures/<task-id>/ so npm test starts red ONLY on
  // the target's own test — cross-task contamination is impossible.
  fs.rmSync(WORK_DIR, { recursive: true, force: true });
  fs.cpSync(WORK_SRC_DIR, WORK_DIR, { recursive: true });
  const fixture = path.join(FIXTURES_DIR, taskId);
  if (fs.existsSync(fixture)) {
    fs.cpSync(fixture, WORK_DIR, { recursive: true });
  }
}

async function runTask(base, task, outDir) {
  resetWork(task.id);
  const startedAt = new Date().toISOString();
  const session = await rpc(base, "session/new", {
    workspace: WORK_DIR,
    system_prompt:
      "You are running a benchmark task. Follow the task brief exactly, meet every " +
      "acceptance point, verify with real commands, and do not expand scope. " +
      "When done, give a concise completion report listing which acceptance " +
      "points were verified and with what commands.",
  });
  const promptText = `任务：${task.prompt}\n\nAcceptance：\n${task.acceptance}`;
  const { promptId } = await rpc(base, "prompt/stream", {
    session_id: session.sessionId,
    content: promptText,
  });
  const { terminal, transcript } = await collectPrompt(
    base,
    session.sessionId,
    promptId,
    TASK_TIMEOUT_MS,
  );
  // Archive independent evidence BEFORE closing: `session/close` deletes
  // the session (and its messages/agent_events cascade). The evidence
  // bundle powers per-task scoring without trusting the agent's summary.
  let evidence = null;
  try {
    evidence = await rpc(base, "session/evidence", {
      session_id: session.sessionId,
    });
  } catch (e) {
    console.log(`evidence archive failed: ${e.message}`);
  }
  await rpc(base, "session/close", { session_id: session.sessionId }).catch(() => {});

  const record = {
    id: task.id,
    prompt: promptText,
    started_at: startedAt,
    finished_at: new Date().toISOString(),
    state: terminal?.state ?? "timeout",
    error: terminal?.error ?? null,
    transcript,
    evidence_counts: evidence
      ? {
          events: evidence.events?.length ?? 0,
          messages: evidence.messages?.length ?? 0,
        }
      : null,
    failure: null, // filled by scoring: incomplete|timeout|permission_stuck|wrong_output|laziness|env
    verdict: extractVerdict(transcript),
  };
  const file = path.join(outDir, `${task.id}.json`);
  await writeFile(file, JSON.stringify(record, null, 2), "utf8");
  if (evidence) {
    await writeFile(
      path.join(outDir, `${task.id}.evidence.json`),
      JSON.stringify(evidence, null, 2),
      "utf8",
    );
  }
  return record;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const md = await readFile(TASKS_FILE, "utf8");
  let tasks = parseTasks(md);
  if (args.task) {
    const wanted = new Set(args.task.split(",").map((s) => s.trim()).filter(Boolean));
    tasks = tasks.filter((t) => wanted.has(t.id));
    if (tasks.length === 0) throw new Error(`Unknown task(s): ${args.task}`);
  }
  const outDir = args.out ?? path.join(HERE, "results", `run-${new Date().toISOString().slice(0, 10)}`);
  await mkdir(outDir, { recursive: true });

  console.log(`Running ${tasks.length} code tasks against ${args.base}`);
  console.log(`Output: ${outDir}\n`);
  const results = [];
  for (const task of tasks) {
    process.stdout.write(`[${task.id}] `);
    try {
      const record = await runTask(args.base, task, outDir);
      console.log(record.state);
      results.push(record);
    } catch (e) {
      console.log(`ERROR: ${e.message}`);
      results.push({ id: task.id, state: "error", error: e.message, verdict: null });
    }
  }
  const done = results.filter((r) => r.state === "completed").length;
  const passed = results.filter((r) => r.verdict === "PASS").length;
  const failedVerdicts = results.filter((r) => r.verdict === "FAIL").length;
  const failed = results.filter((r) => r.state === "failed" || r.state === "timeout" || r.state === "error").length;
  console.log(
    `\nSummary: ${done} completed (${passed} PASS / ${failedVerdicts} FAIL by evaluator), ` +
      `${failed} failed/timeout, ${results.length} total`,
  );
  await writeFile(
    path.join(outDir, "summary.json"),
    JSON.stringify({ base: args.base, run_at: new Date().toISOString(), results }, null, 2),
    "utf8",
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
