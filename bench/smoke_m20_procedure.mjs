#!/usr/bin/env node
/**
 * M20 procedural-memory smoke: real app + real key.
 *
 * 1. Start the debug app (hidden) and wait for the ACP endpoint.
 * 2. Session A (code, bypass): complete a tiny task, then call
 *    procedure_save to persist a smoke workflow into the temp workspace.
 * 3. Assert the project procedures.md contains the workflow.
 * 4. Session B (same workspace): procedure_search must find it and the
 *    reply must restate the steps (injection/search closed loop).
 *
 * Usage: node bench/smoke_m20_procedure.mjs
 */

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const APP = path.join(HERE, "..", "src-tauri", "target", "debug", "deepdepcat.exe");
const BASE = process.env.BASE ?? "http://127.0.0.1:31524";
const TIMEOUT_MS = 8 * 60 * 1000;
const MARK = `smoke-m20-${Date.now()}`;

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
  const toolCalls = transcript
    .filter((e) => e.payload?.toolCalls || e.payload?.tool_calls)
    .map((e) => JSON.stringify(e.payload));
  return { state: terminal?.state, text, error: terminal?.error ?? null, toolCalls };
}

function assert(cond, message) {
  if (!cond) throw new Error(`ASSERT FAILED: ${message}`);
}

async function main() {
  const child = spawn(APP, [], { stdio: "ignore", windowsHide: true });
  let appStarted = false;
  const tmp = mkdtempSync(path.join(os.tmpdir(), "ddc-m20-"));
  let sessionA = null;
  let sessionB = null;
  try {
    await waitReady();
    appStarted = true;

    const saveTask = `完成一个最小演示任务：在工作区创建 demo.md，内容为 "M20 smoke ok"。\n任务验证通过后，必须调用 procedure_save 保存这条工作流：name='${MARK}'，trigger='M20 冒烟'，steps=['创建 demo.md','验证文件内容'], verify=['demo.md 内容正确'], mode='code'，scope='project'。\n调用后再给出完成报告。`;
    sessionA = await runSession(
      tmp,
      "You are running a memory smoke test. Follow the brief exactly. Do not skip the procedure_save call.",
      saveTask,
    );
    assert(sessionA.state === "completed", `session A state=${sessionA.state} ${sessionA.error ?? ""}`);

    const proceduresPath = path.join(tmp, ".deepdepcat", "procedures.md");
    const content = readFileSync(proceduresPath, "utf8");
    assert(content.includes(`## procedure: ${MARK}`), "procedures.md missing saved workflow");
    assert(content.includes("创建 demo.md"), "saved workflow missing step text");

    const recallTask = `用 procedure_search 查询 "M20 冒烟"，然后复述找到的流程的完整步骤。不要做其他修改。`;
    sessionB = await runSession(
      tmp,
      "You are running a memory recall smoke test. Search and restate, nothing else.",
      recallTask,
    );
    assert(sessionB.state === "completed", `session B state=${sessionB.state} ${sessionB.error ?? ""}`);
    const recalled = sessionB.text.includes("创建 demo.md") || sessionB.text.includes("验证文件内容");
    assert(recalled, `session B did not recall the procedure: ${sessionB.text.slice(0, 800)}`);

    console.log(`M20 PROCEDURE MEMORY SMOKE PASS: saved=${MARK}, recalled=true`);
    console.log(`workspace: ${tmp}`);
  } catch (e) {
    console.error(`SMOKE FAILED: ${e.message}`);
    console.error(`workspace kept for inspection: ${tmp}`);
    const diag = {
      sessionA: {
        state: sessionA?.state,
        error: sessionA?.error ?? null,
        textTail: sessionA?.text?.slice(-1500) ?? null,
        toolCalls: sessionA?.toolCalls ?? [],
      },
      sessionB: {
        state: sessionB?.state,
        error: sessionB?.error ?? null,
        textTail: sessionB?.text?.slice(-800) ?? null,
      },
    };
    const { writeFileSync } = await import("node:fs");
    writeFileSync(path.join(tmp, "smoke-diagnostic.json"), JSON.stringify(diag, null, 2), "utf8");
    process.exitCode = 1;
    return;
  } finally {
    if (appStarted) child.kill();
  }
}

main().catch((e) => {
  console.error(e.message ?? e);
  process.exit(1);
});
