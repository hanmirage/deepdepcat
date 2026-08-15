#!/usr/bin/env node
/**
 * M20 phase-2 smoke: automatic procedural-memory capture (real app + key).
 *
 * Unlike smoke_m20_procedure.mjs (explicit procedure_save), this run NEVER
 * mentions procedure_save: the agent just completes a small multi-step
 * task, and the background capture must distill a workflow into the
 * project procedures.md on its own. A second session must then recall it.
 *
 * Usage: node bench/smoke_m20_auto_procedure.mjs
 */

import { spawn } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  existsSync,
  rmSync,
  writeFileSync,
  openSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const APP = path.join(HERE, "..", "src-tauri", "target", "debug", "deepdepcat.exe");
const BASE = process.env.BASE ?? "http://127.0.0.1:31524";
const TIMEOUT_MS = 8 * 60 * 1000;
const CAPTURE_WAIT_MS = 120_000;

async function rpc(method, params, timeoutMs = 30_000) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(`${BASE}/rpc`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: crypto.randomUUID(), method, params }),
      signal: controller.signal,
    });
    const body = await res.json();
    if (body.error) {
      throw new Error(`${method}: ${body.error.message ?? JSON.stringify(body.error)}`);
    }
    return body.result;
  } finally {
    clearTimeout(timer);
  }
}

async function waitReady() {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    try {
      await rpc("session/evidence", { session_id: "__probe__" }, 3_000);
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 1_500));
    }
  }
  throw new Error("ACP endpoint did not come up in 90s");
}

async function portInUse() {
  try {
    await rpc("session/evidence", { session_id: "__probe__" }, 2_000);
    return true;
  } catch {
    return false;
  }
}

async function collectPrompt(sessionId, promptId, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const res = await fetch(`${BASE}/events`, { signal: controller.signal });
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
        let event = "message", data = "";
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
        if (
          event === "prompt/update" &&
          payload.promptId === promptId &&
          (payload.state === "completed" || payload.state === "failed")
        ) {
          terminal = payload;
          controller.abort();
          break;
        }
      }
      if (terminal) break;
    }
  } catch {
    // Abort on terminal is expected.
  }
  return { terminal, transcript };
}

async function runSession(workspace, systemPrompt, content) {
  const session = await rpc("session/new", {
    workspace,
    work_mode: "code",
    permission_mode: "bypass",
    system_prompt: systemPrompt,
  });
  const { promptId } = await rpc("prompt/stream", {
    session_id: session.sessionId,
    content,
  });
  const { terminal, transcript } = await collectPrompt(
    session.sessionId,
    promptId,
    TIMEOUT_MS,
  );
  await rpc("session/close", { session_id: session.sessionId }).catch(() => {});
  const text = transcript
    .filter((e) => e.payload?.message?.role === "assistant" || e.payload?.text)
    .map((e) => e.payload?.message?.content ?? e.payload?.text ?? "")
    .join("\n");
  return { state: terminal?.state, text, error: terminal?.error ?? null };
}

function assert(cond, message) {
  if (!cond) throw new Error(`ASSERT FAILED: ${message}`);
}

async function waitForCapture(proceduresPath, needle) {
  const deadline = Date.now() + CAPTURE_WAIT_MS;
  while (Date.now() < deadline) {
    if (existsSync(proceduresPath)) {
      const content = readFileSync(proceduresPath, "utf8");
      if (content.includes(needle)) return content;
    }
    await new Promise((r) => setTimeout(r, 2_000));
  }
  throw new Error(`procedures.md was not auto-captured within ${CAPTURE_WAIT_MS / 1000}s`);
}

async function main() {
  const tmp = mkdtempSync(path.join(os.tmpdir(), "ddc-m20auto-"));
  const appLog = path.join(tmp, "app.log");
  const logFd = openSync(appLog, "a");
  // Reuse an already-running app (e.g. a manual instance started for
  // debugging); only spawn our own when the ACP port is free.
  const existing = await portInUse();
  const child = existing
    ? null
    : spawn(APP, [], { stdio: ["ignore", logFd, logFd], windowsHide: true });
  let appStarted = !existing;
  let sessionA = null;
  let sessionB = null;
  try {
    await waitReady();
    appStarted = true;

    const task = `完成一个多步小任务（全部在工作区进行）：\n1. 创建 README.md，写 2 行项目简介；\n2. 创建 src/data.ts，写一个含 3 个字符串的数组并导出；\n3. 用 node 检查 src/data.ts 语法正确；\n全部完成并验证后，给出简洁完成报告（列出产物与验证结果）。`;
    sessionA = await runSession(
      tmp,
      "You are running a memory smoke test. Complete the task, nothing else.",
      task,
    );
    assert(
      sessionA.state === "completed",
      `session A state=${sessionA.state} ${sessionA.error ?? ""}`,
    );

    const proceduresPath = path.join(tmp, ".deepdepcat", "procedures.md");
    const content = await waitForCapture(proceduresPath, "README");
    assert(
      content.includes("README") && content.includes("data.ts"),
      `captured workflow missing expected steps: ${content}`,
    );

    const recallTask = `用 procedure_search 查询 "README"，然后复述找到的流程的完整步骤。不要做其他修改。`;
    sessionB = await runSession(
      tmp,
      "You are running a memory recall smoke test. Search and restate, nothing else.",
      recallTask,
    );
    assert(
      sessionB.state === "completed",
      `session B state=${sessionB.state} ${sessionB.error ?? ""}`,
    );
    const recalled =
      sessionB.text.includes("README") && sessionB.text.includes("data.ts");
    assert(recalled, `session B did not recall the procedure: ${sessionB.text.slice(0, 800)}`);

    console.log("M20 AUTO-PROCEDURE SMOKE PASS: background capture + recall closed loop");
  } catch (e) {
    console.error(`SMOKE FAILED: ${e.message}`);
    const diag = {
      sessionA: {
        state: sessionA?.state,
        error: sessionA?.error ?? null,
        textTail: sessionA?.text?.slice(-1200) ?? null,
      },
      sessionB: {
        state: sessionB?.state,
        error: sessionB?.error ?? null,
        textTail: sessionB?.text?.slice(-800) ?? null,
      },
    };
    writeFileSync(path.join(tmp, "smoke-diagnostic.json"), JSON.stringify(diag, null, 2), "utf8");
    console.error(`app log: ${appLog}`);
    console.error(`workspace kept for inspection: ${tmp}`);
    process.exitCode = 1;
    return;
  } finally {
    if (appStarted && child) child.kill();
    if (process.exitCode === undefined) rmSync(tmp, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error(e.message ?? e);
  process.exit(1);
});
