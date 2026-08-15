import { logError } from "@/lib/logger";

/**
 * EnergySliderShader — WebGL2 cell-energy particle renderer.
 *
 * Ported from Claude Desktop's effort slider. A horizontal bar of small rounded
 * cells lights up when "energy" bursts fire: each burst is a wavefront racing
 * outward in Manhattan distance, cells flip on as the front reaches them, then
 * cool through a discrete 5-step ramp. A charged surface arrives with the same
 * front and stays until the press fades.
 *
 * The bar is invisible at rest — energy IS visibility.
 */

// ── Vertex shader ────────────────────────────────────────────
const VERTEX_SHADER = /* glsl */ `#version 300 es
in vec2 a_position;
out vec2 v_uv;
uniform vec2 u_resolution;
void main() {
  // pixel-space UV so fragment can do floor(uv / gap) without aspect distortion
  v_uv = (a_position * 0.5 + 0.5) * u_resolution;
  gl_Position = vec4(a_position, 0.0, 1.0);
}
`;

// ── Fragment shader (full port of the original) ─────────────
const FRAGMENT_SHADER = /* glsl */ `#version 300 es
precision highp float;
uniform vec2 u_resolution;
uniform float u_time;
uniform vec3 u_fg;   // hot ink — the brightest thing in the bar
uniform vec3 u_fg2;  // cool ink — the low-energy end of the ramp
uniform float u_seed; // re-rolled every press

const int MAX_BURSTS = 8;
uniform float u_burstTime[MAX_BURSTS];
uniform vec2  u_burstCenter[MAX_BURSTS];
uniform float u_burstGain[MAX_BURSTS];   // strength k ∈ [0,1]

uniform float u_fade;   // 1 while pressed, eased to 0 after release

uniform float u_pos;    // handle fraction (0..1)
uniform vec4 u_charge0;
uniform vec4 u_charge1;

uniform float u_bedFill;   // charge spills into gaps (light mode)
uniform vec4 u_tintA;      // accent blue lean
uniform vec4 u_tintB;      // extended pink lean

in vec2 v_uv;
out vec4 fragColor;

// ─── the grid ────────────────────────────────────────────────
const float CELL_PITCH = 4.0;
const float CELL_SIZE  = 3.0;
const float CELL_R     = 0.9;

// ─── the energy ──────────────────────────────────────────────
const float SPEED_LO   = 20.0;
const float SPEED_HI   = 36.0;
const float RANGE_LO   = 10.0;
const float RANGE_HI   = 55.0;
const float DENSITY_LO = 0.30;
const float DENSITY_HI = 0.85;
const float JITTER     = 0.10;
const float HEAD_GAIN  = 1.25;
const float HEAD_DECAY = 5.0;
const float BODY_GAIN  = 0.65;
const float BODY_LO    = 3.3;
const float BODY_HI    = 0.77;
const float AMP_LO     = 0.55;
const float MAX_AGE    = 5.0;

// ─── rendering ───────────────────────────────────────────────
const float RAMP_POW    = 1.4;
const float BREATH_DIP  = 0.28;
const float BREATH_BASE = 2.4;
const float BREATH_VARY = 1.4;

uniform float u_chargeMax;
uniform float u_chargeRamp;
uniform float u_inkFloor;
uniform float u_inkCeil;
uniform float u_energyGain;

float hash21(vec2 p) {
  return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

void main() {
  float pitch = u_resolution.y / max(round(u_resolution.y / CELL_PITCH), 1.0);
  float cellScale = pitch / CELL_PITCH;
  vec2 cell = floor(v_uv / pitch);
  vec2 cellCenter = (cell + 0.5) * pitch;

  float cellHalf = CELL_SIZE * 0.5 * cellScale;
  float cellR = CELL_R * cellScale;
  vec2 q = abs(v_uv - cellCenter) - vec2(cellHalf - cellR);
  float distC = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - cellR;
  float aa = min(fwidth(v_uv.x), 1.5);
  float cellMask = 1.0 - smoothstep(-aa, aa, distC);
  if (cellMask <= 0.0 && u_bedFill <= 0.0) {
    fragColor = vec4(0.0);
    return;
  }

  float jitter = (hash21(cell + 41.7 + u_seed) - 0.5) * 2.0 * JITTER;
  float vary   = 0.35 + 0.65 * hash21(cell + 13.7 + u_seed);

  float energy = 0.0;
  float charged = 0.0;
  for (int i = 0; i < MAX_BURSTS; i++) {
    float age = u_time - u_burstTime[i];
    if (age < 0.0 || age > MAX_AGE) continue;
    float k = clamp(u_burstGain[i], 0.0, 1.0);

    vec2 originCell = floor(u_burstCenter[i] / pitch);
    float manh = abs(cell.x - originCell.x) + abs(cell.y - originCell.y);

    float range   = mix(RANGE_LO, RANGE_HI, k);
    float falloff = exp(-manh / (range * 0.85));
    float reach   = exp(-max(manh - range, 0.0) * 0.12);
    float speed   = mix(SPEED_LO, SPEED_HI, k);
    float t = age - manh / speed - jitter;
    charged = max(charged, smoothstep(0.0, 0.15, t) * reach);
    if (t < 0.0) continue;

    float rank = hash21(cell + 7.3 + u_seed + u_burstTime[i]);
    if (rank > mix(DENSITY_LO, DENSITY_HI, k)) continue;

    float head = HEAD_GAIN * exp(-t * HEAD_DECAY);
    float body = BODY_GAIN * exp(-t * mix(BODY_LO, BODY_HI, k));
    energy += (head + body) * mix(AMP_LO, 1.0, k) * falloff * reach * vary;
  }

  float lumA = hash21(cell + 8.8 + u_seed);
  float lumB = hash21(cell + 88.8 + u_seed);
  float lumDrift = 0.5 + 0.5 * sin(u_time * 0.35 + lumA * 6.2832);
  float lum = pow(mix(lumA, lumB, lumDrift), 1.9);
  float stream = 0.9 + 0.16 * sin(u_time * 1.3 + cell.x * 0.45 + lumB * 2.0);
  float e = clamp(energy, 0.0, 1.0) * u_fade * mix(0.16, 1.0, lum) * stream
    * u_energyGain;

  float g = clamp(v_uv.x / max(u_pos * u_resolution.x, 1.0), 0.0, 1.0);
  vec3 chargeColor = mix(u_charge0.rgb, u_charge1.rgb, g);
  float bedMask = mix(u_bedFill, 1.0, cellMask);
  float chargeA = u_chargeMax * pow(g, u_chargeRamp) * charged * u_fade
    * bedMask * mix(u_charge0.a, u_charge1.a, g);

  float lv = step(0.04, e) + step(0.2, e) + step(0.4, e) + step(0.6, e) + step(0.8, e);
  float alpha = lv <= 0.0 ? 0.0 : mix(u_inkFloor, u_inkCeil, (lv - 1.0) / 4.0);

  if (lv >= 5.0) {
    float period = BREATH_BASE + BREATH_VARY * hash21(cell + 3.1 + u_seed);
    float phase = hash21(cell + 5.5 + u_seed) * 6.2832;
    alpha *= 1.0 - BREATH_DIP * 0.5 * (1.0 + sin(u_time * 6.2832 / period + phase));
  }

  vec3 ink = mix(u_fg2, u_fg, pow(lv / 5.0, RAMP_POW));
  float hueSel = hash21(cell + 27.9 + u_seed);
  ink = mix(ink, u_tintA.rgb, u_tintA.a * smoothstep(0.62, 0.95, hueSel));
  ink = mix(ink, u_tintB.rgb, u_tintB.a * (1.0 - smoothstep(0.05, 0.38, hueSel)));
  float eA = alpha * cellMask;
  vec3 col = ink * eA + chargeColor * chargeA * (1.0 - eA);
  float a = eA + chargeA * (1.0 - eA);
  if (a <= 0.0) {
    fragColor = vec4(0.0);
    return;
  }
  fragColor = vec4(col, a);
}
`;

/** Full-screen triangle for the shader. */
const VERTICES = new Float32Array([-1, -1, 3, -1, -1, 3]);

/**
 * Track every live energy bar. On creation we destroy any previously live bar
 * so at most ONE rAF loop + GL program exists at a time. React StrictMode and
 * Radix popover remounts otherwise leave zombie loops that clear each other's
 * canvas.
 */
const liveBars = new Set<EnergyBarHandle>();

function registerBar(bar: EnergyBarHandle) {
  for (const existing of liveBars) {
    if (existing !== bar) existing.destroy();
  }
  liveBars.clear();
  liveBars.add(bar);
}

export interface EnergyBarHandle {
  /** Fire a burst at normalized position t (0..1) with gain k (0..1). */
  burst: (t: number, k?: number) => void;
  /** Begin a press — sets fade to 1 and re-rolls the seed. */
  press: () => void;
  /** Release — field cools down. */
  release: () => void;
  /** Set the charge handle position (0..1). */
  setPos: (t: number) => void;
  /** Pause the rAF loop (keeps WebGL resources alive). */
  stop: () => void;
  /** Fully tear down WebGL resources. */
  destroy: () => void;
}

export function createEnergyBar(canvas: HTMLCanvasElement): EnergyBarHandle | null {
  const gl = canvas.getContext("webgl2", { alpha: true, antialias: false, depth: false, stencil: false });
  if (!gl) return null;

  // ── Compile shaders ─────────────────────────────────────────
  const compile = (type: number, src: string) => {
    const sh = gl.createShader(type);
    if (!sh) return null;
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      logError("EnergyBar", "shader compile error:", gl.getShaderInfoLog(sh));
      gl.deleteShader(sh);
      return null;
    }
    return sh;
  };
  const vs = compile(gl.VERTEX_SHADER, VERTEX_SHADER);
  const fs = compile(gl.FRAGMENT_SHADER, FRAGMENT_SHADER);
  if (!vs || !fs) {
    gl.deleteShader(vs ?? null);
    gl.deleteShader(fs ?? null);
    return null;
  }

  const prog = gl.createProgram();
  if (!prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    logError("EnergyBar", "link error:", gl.getProgramInfoLog(prog));
    gl.deleteProgram(prog);
    return null;
  }

  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, VERTICES, gl.STATIC_DRAW);
  const aPos = gl.getAttribLocation(prog, "a_position");
  gl.enableVertexAttribArray(aPos);
  gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

  const loc = (name: string) => gl.getUniformLocation(prog, name);
  const uTime = loc("u_time");
  const uRes = loc("u_resolution");
  const uSeed = loc("u_seed");
  const uPos = loc("u_pos");
  const uCharge0 = loc("u_charge0");
  const uCharge1 = loc("u_charge1");
  const uBedFill = loc("u_bedFill");
  const uChargeMax = loc("u_chargeMax");
  const uChargeRamp = loc("u_chargeRamp");
  const uInkFloor = loc("u_inkFloor");
  const uInkCeil = loc("u_inkCeil");
  const uEnergyGain = loc("u_energyGain");
  const uTintA = loc("u_tintA");
  const uTintB = loc("u_tintB");
  const uFg = loc("u_fg");
  const uFg2 = loc("u_fg2");
  const uBurstTime = loc("u_burstTime[0]");
  const uBurstCenter = loc("u_burstCenter[0]");
  const uBurstGain = loc("u_burstGain[0]");
  const uFade = loc("u_fade");

  gl.clearColor(0, 0, 0, 0);
  // Default palette — a TRUE saturated purple (violet-500 ≈ #8b5cf6, matching
  // the track fill). The earlier lavender (high B) read as blue.
  gl.uniform3fv(uFg, [0.55, 0.36, 0.96]);
  gl.uniform3fv(uFg2, [1, 1, 1]);
  gl.uniform1f(uFade, 0);
  gl.uniform1f(uSeed, 0);
  gl.uniform1f(uPos, 1);
  gl.uniform4f(uCharge0, 0.55, 0.36, 0.96, 1);
  gl.uniform4f(uCharge1, 0.34, 0.2, 0.68, 1);
  gl.uniform1f(uBedFill, 0);
  gl.uniform1f(uChargeMax, 0.9);
  gl.uniform1f(uChargeRamp, 1.1);
  gl.uniform1f(uInkFloor, 0.1);
  gl.uniform1f(uInkCeil, 0.95);
  gl.uniform1f(uEnergyGain, 1.15);
  gl.uniform4f(uTintA, 0, 0, 0, 0);
  gl.uniform4f(uTintB, 0, 0, 0, 0);

  const burstTime = new Float32Array(8).fill(-1e3);
  const burstCenter = new Float32Array(16);
  const burstGain = new Float32Array(8);
  gl.uniform1fv(uBurstTime, burstTime);
  gl.uniform2fv(uBurstCenter, burstCenter);
  gl.uniform1fv(uBurstGain, burstGain);

  // ── State ───────────────────────────────────────────────────
  let width = 1, height = 1, dpr = 1;
  let fade = 0;       // master envelope
  let pressed = false;
  const startTime = performance.now();
  let time = 0;       // shader u_time clock (seconds since startTime)
  let lastRafNow = startTime;
  let pressClock = 0; // time since press start
  let burstIdx = 0;
  let rafId: number | null = null;
  let frames = 0;
  let alive = true;
  let pos = 1;
  let seed = 0;
  let dirty = true;

  /** Seconds since the bar was created — the shared clock for u_time and bursts. */
  const clock = () => (performance.now() - startTime) / 1000;

  const resize = (w: number, h: number, ratio: number) => {
    w = Math.max(1, Math.round(w));
    h = Math.max(1, Math.round(h));
    if (w !== width || h !== height || ratio !== dpr) {
      if (w !== width || h !== height) {
        for (let n = 0; n < 8; n++) {
          burstCenter[2 * n] *= w / Math.max(width, 1);
          burstCenter[2 * n + 1] *= h / Math.max(height, 1);
        }
      }
      width = w; height = h; dpr = ratio;
      dirty = true;
    }
    let cw = Math.max(1, Math.round(w * ratio));
    let ch = Math.max(1, Math.round(h * ratio));
    const cap = 8192 * 8192;
    const d = cw * ch;
    if (d > cap) {
      const s = Math.sqrt(cap / d);
      cw = Math.max(1, Math.round(cw * s));
      ch = Math.max(1, Math.round(ch * s));
    }
    if (canvas.width !== cw || canvas.height !== ch) {
      canvas.width = cw; canvas.height = ch;
      dirty = true;
    }
  };

  const render = () => {
    if (dirty) {
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform2f(uRes, width * dpr, height * dpr);
      dirty = false;
    }
    gl.uniform1f(uTime, time);
    gl.uniform1f(uPos, Math.max(pos, 0.02));
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
  };
  const frame = () => {
    if (!alive) {
      rafId = null;
      return;
    }
    frames++;
    const now = performance.now();
    time = (now - startTime) / 1000;
    if (pressed) {
      pressClock += (now - lastRafNow) / 1000;
      fade = Math.min(fade + (now - lastRafNow) / 700, 1);
    } else {
      fade = Math.max(fade - (now - lastRafNow) / 850, 0);
    }
    lastRafNow = now;
    // While pressed, keep re-firing bursts so the field keeps tracing.
    if (pressed && time - lastBurstClock > 0.45) {
      const k = Math.min(pressClock / 1.6, 1);
      fire(pos + 0.08 * (Math.random() - 0.5), k);
    }
    gl.uniform1f(uFade, fade);
    render();

    // Cool-down exit — the field is invisible at rest, so idle bars must not
    // burn 60fps. Stop once there's no press, no remaining envelope, and no
    // burst still travelling (lastBurstClock starts at 0, so the 1.5s window
    // is only armed after the first real burst). burst()/press() restart.
    const neverFired = fireCount === 0;
    const idle = !pressed && fade <= 0.0001 && (neverFired || time - lastBurstClock > 1.5);
    if (idle) {
      rafId = null;
      return;
    }
    rafId = requestAnimationFrame(frame);
  };
  if (import.meta.env.DEV) {
    (window as unknown as Record<string, unknown>).__energyBarDebug = {
      get pressed() { return pressed; },
      get fade() { return fade; },
      get fireCount() { return fireCount; },
      get frameCount() { return frames; },
      get rafActive() { return rafId != null; },
    };
  }

  let lastBurstClock = 0;
  let fireCount = 0;
  const fire = (t: number, k: number) => {
    fireCount++;
    burstTime[burstIdx] = clock();  // absolute seconds — matches u_time
    burstCenter[2 * burstIdx] = t * (width * dpr);
    burstCenter[2 * burstIdx + 1] = (height * dpr) * (0.35 + 0.3 * Math.random());
    burstGain[burstIdx] = Math.min(Math.max(k, 0), 1);
    burstIdx = (burstIdx + 1) % 8;
    gl.uniform1fv(uBurstTime, burstTime);
    gl.uniform2fv(uBurstCenter, burstCenter);
    gl.uniform1fv(uBurstGain, burstGain);
    lastBurstClock = time;
  };

  const start = () => {
    if (rafId != null) return;
    lastRafNow = performance.now();
    rafId = requestAnimationFrame(frame);
  };

  start();

  // ── Size management ────────────────────────────────────────
  // The energy field needs the canvas's real CSS size as its pixel-space
  // resolution. Set it once and keep it in sync with resize events.
  const ownerDoc = canvas.ownerDocument;
  const view = ownerDoc.defaultView ?? window;
  const applySize = () => {
    const rect = canvas.getBoundingClientRect();
    const ratio = view.devicePixelRatio || 1;
    if (rect.width > 0 && rect.height > 0) {
      resize(rect.width, rect.height, ratio);
    }
  };
  applySize();
  const ro = new ResizeObserver(() => applySize());
  ro.observe(canvas);

  const handle: EnergyBarHandle = {
    burst: (t, k = 1) => {
      fire(Math.min(Math.max(t, 0), 1), k);
      // Simulate a press pulse: jump the master envelope to 1 so the burst is
      // visible even on a single click (drag presses drive it via press()).
      fade = 1;
      time = clock();
      gl.uniform1f(uFade, fade);
      gl.uniform1f(uTime, time);
      // Render synchronously so the burst appears immediately on click,
      // not a frame late (and before fade cools down).
      render();
      start();
    },
    press: () => {
      pressed = true;
      pressClock = 0;
      seed = Math.random();
      gl.uniform1f(uSeed, seed);
      start();
    },
    release: () => { pressed = false; },
    setPos: (t) => { pos = Math.min(Math.max(t, 0), 1); },
    stop: () => {
      if (rafId != null) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
    },
    destroy: () => {
      alive = false;
      liveBars.delete(handle);
      if (rafId != null) cancelAnimationFrame(rafId);
      rafId = null;
      ro.disconnect();
      gl.deleteBuffer(buf);
      gl.deleteProgram(prog);
    },
  };
  registerBar(handle);
  return handle;
}
