// Scroll reveal
const obs = new IntersectionObserver((entries) => {
  entries.forEach((e) => { if (e.isIntersecting) { e.target.classList.add('in'); obs.unobserve(e.target); } });
}, { threshold: 0.12 });
document.querySelectorAll('.reveal').forEach((el, i) => {
  el.style.transitionDelay = (i % 4) * 60 + 'ms';
  obs.observe(el);
});

// Animated "Listening..." dots
const anim = document.querySelector('.anim-dots');
if (anim) {
  const states = ['', '.', '..', '...'];
  let k = 0;
  setInterval(() => { anim.textContent = states[k = (k + 1) % states.length]; }, 450);
}

// Waveform audio visualizers (one per demo panel)
document.querySelectorAll('.waveform').forEach((wf) => {
  const N = 24;
  // Only 2-3 bars per waveform glow
  const glowCount = 2 + Math.floor(Math.random() * 2);
  const glowSet = new Set();
  while (glowSet.size < glowCount) glowSet.add(Math.floor(Math.random() * N));
  for (let i = 0; i < N; i++) {
    const bar = document.createElement('i');
    // Bell-shaped max height: taller toward the center
    const center = (N - 1) / 2;
    const dist = Math.abs(i - center) / center;
    const base = 92 * (1 - dist * 0.66) + Math.random() * 18;
    bar.style.height = base.toFixed(0) + 'px';
    bar.style.background = 'rgba(255,255,255,' + (0.4 + Math.random() * 0.55).toFixed(2) + ')';
    // Slower cadence
    bar.style.animationDelay = (Math.random() * 1.6).toFixed(2) + 's';
    bar.style.animationDuration = (1.8 + Math.random() * 1.2).toFixed(2) + 's';
    // Glow bars run wave + waveGlow together
    if (glowSet.has(i)) bar.style.animationName = 'wave, waveGlow';
    wf.appendChild(bar);
  }
});

// Typewriter: loop forever
document.addEventListener('DOMContentLoaded', function() {
  const input = document.querySelector('.capture-input[data-typewriter]');
  if (!input) return;
  const typeEl = input.querySelector('.type-text');
  const segs = [
    { kind: 'text', val: '说出你的指令，或按 ' },
    { kind: 'kbd',  val: '/' },
    { kind: 'text', val: ' 唤起智能体命令' },
  ];
  const addKbd = (v, pop) => { const k = document.createElement('kbd'); k.textContent = v; if (pop) k.classList.add('kbd-pop'); typeEl.appendChild(k); };

  // Reduced motion: render once, no animation, no loop.
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
    segs.forEach((s) => s.kind === 'kbd' ? addKbd(s.val, false) : typeEl.appendChild(document.createTextNode(s.val)));
    return;
  }

  // Flatten to steps
  const steps = [];
  segs.forEach((s) => {
    if (s.kind === 'kbd') steps.push({ kind: 'kbd', val: s.val });
    else for (const ch of s.val) steps.push({ kind: 'char', val: ch });
  });

  const LEAD_IN = 650;
  const HOLD    = 4200;
  const FADE    = 550;
  const GAP     = 450;
  let i = 0;

  const type = () => {
    if (i >= steps.length) { input.classList.remove('typing'); setTimeout(fadeOut, HOLD); return; }
    const step = steps[i++];
    if (step.kind === 'kbd') addKbd(step.val, true);
    else typeEl.appendChild(document.createTextNode(step.val));
    let d = 95 + Math.random() * 55;
    if ('，。？！'.includes(step.val)) d += 260;
    if (step.kind === 'kbd') d += 120;
    setTimeout(type, d);
  };

  const fadeOut = () => {
    input.classList.add('fading');
    setTimeout(() => {
      typeEl.textContent = '';
      input.classList.remove('fading');
      i = 0;
      input.classList.add('typing');
      setTimeout(type, GAP);
    }, FADE);
  };

  input.classList.add('typing');
  setTimeout(type, LEAD_IN);
});

// Feature cards: cursor-follow 3D tilt + spotlight
(function () {
  const cards = document.querySelectorAll('.feature');
  if (!cards.length) return;
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const TILT = 15, SCALE = 1.05, PERSP = 1200, DIR = -1;
  cards.forEach((card) => {
    const spot = document.createElement('span');
    spot.className = 'feature-spotlight';
    spot.setAttribute('aria-hidden', 'true');
    card.appendChild(spot);
    const neutral = 'perspective(' + PERSP + 'px) rotateX(0deg) rotateY(0deg) scale3d(1,1,1)';
    card.addEventListener('pointerenter', () => {
      card.classList.add('tilt-on');
      if (!reduce) card.style.transition = 'transform .18s ease-out';
    });
    card.addEventListener('pointermove', (e) => {
      const r = card.getBoundingClientRect();
      const px = (e.clientX - r.left) / r.width;
      const py = (e.clientY - r.top) / r.height;
      spot.style.setProperty('--sx', (px * 100).toFixed(1) + '%');
      spot.style.setProperty('--sy', (py * 100).toFixed(1) + '%');
      if (reduce) return;
      const xRot = (py - 0.5) * (TILT * 2) * DIR;
      const yRot = (px - 0.5) * -(TILT * 2) * DIR;
      card.style.transform = 'perspective(' + PERSP + 'px) rotateX(' + xRot.toFixed(2) + 'deg) rotateY(' + yRot.toFixed(2) + 'deg) scale3d(' + SCALE + ',' + SCALE + ',' + SCALE + ')';
    });
    card.addEventListener('pointerleave', () => {
      card.classList.remove('tilt-on');
      if (!reduce) card.style.transform = neutral;
    });
    card.addEventListener('transitionend', (e) => {
      if (e.propertyName === 'transform' && !card.classList.contains('tilt-on')) {
        card.style.transition = '';
        card.style.transform = '';
      }
    });
  });
})();

// Orb: procedural 3D particle sphere
(function() {
  const wrap = document.querySelector('.hero-orb');
  const canvas = document.getElementById('orb-canvas');
  if (!wrap || !canvas) return;
  const ctx = canvas.getContext('2d');
  let W = 0, H = 0, dpr = 1, CX = 0, CY = 0, RADIUS = 0, CAM = 0;
  let particles = [];
  let strays = [];

  const SHELL_COUNT = 2600;
  const WAVE_COUNT  = 1900;
  const ROT_SPEED   = 0.00008;
  const TILT        = 0.32;
  const WAVE_BAND   = 0.095;
  const WAVE_LAT    = -0.5;
  const WAVE_AMP    = 0.62;
  const SHELL_SHIMMER = 0.02;
  const FLOW_SPEED  = 0.0009;

  const STRAY_COUNT = 90;
  const STRAY_DRIFT = 0.05;
  const STRAY_EXCL  = 1.2;

  const MOUSE_R = 0.32;
  const PUSH_K  = 0.016;
  const RETURN  = 0.0025;
  const DAMP    = 0.95;
  const MOUSE_LERP = 0.07;
  const mouse = { x: -9999, y: -9999, tx: -9999, ty: -9999 };
  const GOLDEN = 2.399963229728653;

  function resize() {
    dpr = window.devicePixelRatio || 1;
    W = canvas.clientWidth; H = canvas.clientHeight;
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    CX = W / 2; CY = H / 2;
    RADIUS = Math.min(W, H) * 0.255;
    CAM = RADIUS * 3.0;
    positionOrb();
    buildStrays();
  }

  const hero = wrap.parentElement;
  const captureBox = document.querySelector('.capture-box');
  function offsetTopWithin(el, ancestor) {
    let y = 0, node = el;
    while (node && node !== ancestor) { y += node.offsetTop; node = node.offsetParent; }
    return y;
  }
  function positionOrb() {
    if (!captureBox || !hero) return;
    const centerY = offsetTopWithin(captureBox, hero) + captureBox.offsetHeight / 2;
    const offset = wrap.offsetHeight * 0.38;
    wrap.style.top = (centerY + offset) + 'px';
  }

  function wave(phi, t) {
    return Math.sin(6 * phi + t) * 0.5
         + Math.sin(11 * phi + 1.7 * t) * 0.3
         + Math.sin(3 * phi - 0.6 * t) * 0.2;
  }

  function build() {
    particles = [];
    for (let i = 0; i < SHELL_COUNT; i++) {
      const u = 1 - (i / (SHELL_COUNT - 1)) * 2;
      const y = Math.sign(u) * Math.pow(Math.abs(u), 0.6);
      const th = i * GOLDEN;
      particles.push({
        type: 0, phi: th, lat: Math.asin(y),
        phase: Math.random() * Math.PI * 2,
        dx: 0, dy: 0, dz: 0, vx: 0, vy: 0, vz: 0
      });
    }
    for (let i = 0; i < WAVE_COUNT; i++) {
      particles.push({
        type: 1,
        phi: (i / WAVE_COUNT) * Math.PI * 2,
        latBase: WAVE_LAT + (Math.random() - 0.5) * 2 * WAVE_BAND,
        jphi: (Math.random() - 0.5) * 0.05,
        phase: Math.random() * Math.PI * 2,
        dx: 0, dy: 0, dz: 0, vx: 0, vy: 0, vz: 0
      });
    }
  }

  function buildStrays() {
    strays = [];
    const excl = RADIUS * STRAY_EXCL;
    for (let i = 0; i < STRAY_COUNT; i++) {
      let x, y, tries = 0;
      do {
        x = Math.random() * W;
        y = Math.random() * H;
      } while (Math.hypot(x - CX, y - CY) < excl && ++tries < 24);
      strays.push({
        x, y,
        vx: (Math.random() - 0.5) * 2 * STRAY_DRIFT,
        vy: (Math.random() - 0.5) * 2 * STRAY_DRIFT,
        size: 1.1 + Math.random() * 1.6,
        base: 0.4 + Math.random() * 0.5,
        tw: Math.random() * Math.PI * 2,
        tws: 0.0006 + Math.random() * 0.0016
      });
    }
  }

  function loop(now) {
    mouse.x += (mouse.tx - mouse.x) * MOUSE_LERP;
    mouse.y += (mouse.ty - mouse.y) * MOUSE_LERP;

    const ang = now * ROT_SPEED;
    const cosY = Math.cos(ang), sinY = Math.sin(ang);
    const cosX = Math.cos(TILT), sinX = Math.sin(TILT);
    const t = now * FLOW_SPEED;

    const active = mouse.tx > -9000;
    let mlx = 0, mly = 0, mlz = 0;
    if (active) {
      const mvx = (mouse.x - CX) / RADIUS;
      const mvy = (mouse.y - CY) / RADIUS;
      const r2 = mvx * mvx + mvy * mvy;
      const mvz = r2 < 1 ? -Math.sqrt(1 - r2) : 0;
      const yy = mvy * cosX + mvz * sinX;
      const z1 = -mvy * sinX + mvz * cosX;
      mlx = mvx * cosY - z1 * sinY;
      mly = yy;
      mlz = mvx * sinY + z1 * cosY;
    }

    ctx.clearRect(0, 0, W, H);
    ctx.globalCompositeOperation = 'lighter';

    for (let k = 0; k < particles.length; k++) {
      const p = particles[k];
      let hx, hy, hz, crest = 0;

      if (p.type === 0) {
        const lat = p.lat + wave(p.phi + p.phase, t) * SHELL_SHIMMER;
        const cl = Math.cos(lat);
        hx = Math.cos(p.phi) * cl; hy = Math.sin(lat); hz = Math.sin(p.phi) * cl;
      } else {
        const phi = p.phi + p.jphi;
        crest = wave(phi, t + p.phase * 0.2);
        const lat = p.latBase + crest * WAVE_AMP * WAVE_BAND * 3;
        const cl = Math.cos(lat);
        hx = Math.cos(phi) * cl; hy = Math.sin(lat); hz = Math.sin(phi) * cl;
      }

      const cx = hx + p.dx, cy = hy + p.dy, cz = hz + p.dz;
      if (active) {
        const ddx = cx - mlx, ddy = cy - mly, ddz = cz - mlz;
        const dist = Math.sqrt(ddx * ddx + ddy * ddy + ddz * ddz);
        if (dist < MOUSE_R && dist > 1e-4) {
          const force = (MOUSE_R - dist) * PUSH_K;
          const inv = 1 / dist;
          p.vx += ddx * inv * force;
          p.vy += ddy * inv * force;
          p.vz += ddz * inv * force;
        }
      }
      p.vx += -p.dx * RETURN; p.vy += -p.dy * RETURN; p.vz += -p.dz * RETURN;
      p.vx *= DAMP; p.vy *= DAMP; p.vz *= DAMP;
      p.dx += p.vx; p.dy += p.vy; p.dz += p.vz;

      const lx = (hx + p.dx) * RADIUS;
      const ly = (hy + p.dy) * RADIUS;
      const lz = (hz + p.dz) * RADIUS;

      const x1 = lx * cosY + lz * sinY;
      const z1 = -lx * sinY + lz * cosY;
      const y1 = ly * cosX - z1 * sinX;
      const z2 = ly * sinX + z1 * cosX;

      const scale = CAM / (CAM + z2);
      const sx = CX + x1 * scale;
      const sy = CY + y1 * scale;

      const depth = (1 - z2 / RADIUS) / 2;
      let size, alpha;
      if (p.type === 0) {
        const pole = Math.abs(hy);
        size = (0.55 + depth * 0.9) * scale;
        alpha = Math.min(1, 0.06 + depth * depth * 0.80 + pole * pole * 0.24);
      } else {
        const c = (crest + 1) / 2;
        size = (0.85 + c * 1.55) * scale;
        alpha = Math.min(1, (0.40 + c * 0.60) * (0.16 + depth * 0.84));
      }
      ctx.fillStyle = 'rgba(255, 255, 255, ' + alpha.toFixed(3) + ')';
      ctx.fillRect(sx, sy, size, size);
    }

    const sExcl = RADIUS * STRAY_EXCL, sBand = RADIUS * 0.22;
    for (let s = 0; s < strays.length; s++) {
      const p = strays[s];
      p.x += p.vx; p.y += p.vy;
      if (p.x < 0) p.x += W; else if (p.x > W) p.x -= W;
      if (p.y < 0) p.y += H; else if (p.y > H) p.y -= H;
      const dist = Math.hypot(p.x - CX, p.y - CY);
      if (dist < sExcl) continue;
      const edge = Math.min(1, (dist - sExcl) / sBand);
      const twinkle = 0.6 + 0.4 * Math.sin(now * p.tws + p.tw);
      const alpha = p.base * twinkle * edge;
      if (alpha <= 0.01) continue;
      ctx.fillStyle = 'rgba(255, 255, 255, ' + alpha.toFixed(3) + ')';
      ctx.fillRect(p.x, p.y, p.size, p.size);
    }

    ctx.globalCompositeOperation = 'source-over';
    requestAnimationFrame(loop);
  }

  function onMove(clientX, clientY) {
    const r = canvas.getBoundingClientRect();
    mouse.tx = clientX - r.left;
    mouse.ty = clientY - r.top;
  }
  document.addEventListener('mousemove', (e) => onMove(e.clientX, e.clientY));
  document.addEventListener('mouseleave', () => { mouse.tx = -9999; mouse.ty = -9999; });
  document.addEventListener('touchmove', (e) => onMove(e.touches[0].clientX, e.touches[0].clientY), { passive: true });
  document.addEventListener('touchend', () => { mouse.tx = -9999; mouse.ty = -9999; });

  window.addEventListener('resize', resize);
  window.addEventListener('load', positionOrb);
  [300, 900, 1600].forEach((d) => setTimeout(positionOrb, d));
  resize();
  build();
  requestAnimationFrame(loop);
})();

// Add Meoo watermark
document.addEventListener('DOMContentLoaded', function() {
  const watermark = document.createElement('div');
  watermark.id = 'meoo-brand-watermark';
  watermark.style.cssText = 'position: fixed; bottom: 20px; right: 20px; z-index: 9999999; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; font-size: 12px; color: rgb(102, 102, 102); background: rgba(255, 255, 255, 0.95); padding: 6px 8px 6px 10px; border-radius: 8px; box-shadow: rgba(0, 0, 0, 0.1) 0px 2px 12px; backdrop-filter: blur(8px); cursor: pointer; transition: 0.2s; user-select: none; display: flex; align-items: center; gap: 5px; transform: scale(1);';
  watermark.innerHTML = `
    <img src="https://assets.cdn.meoo.host/public/meoo-logo.png" alt="Meoo" style="width: 16px; height: 16px; border-radius: 3px;">
    <span style="white-space: nowrap;">By <span style="font-weight: 600; color: #007bff;">Meoo 秒悟</span></span>
    <button onclick="event.stopPropagation(); document.getElementById('meoo-brand-watermark').remove();" style="
      background: none;
      border: none;
      color: #bbb;
      cursor: pointer;
      font-size: 16px;
      font-weight: bold;
      margin-left: 2px;
      padding: 0;
      width: 14px;
      height: 14px;
      display: flex;
      align-items: center;
      justify-content: center;
      transition: color 0.2s ease;
      line-height: 1;
    " onmouseover="this.style.color='#666';" onmouseout="this.style.color='#bbb';">×</button>
  `;
  watermark.onclick = () => window.open('https://meoo.com', '_blank');
  watermark.onmouseover = () => {
    watermark.style.transform = 'scale(1.02)';
    watermark.style.background = 'rgba(255, 255, 255, 0.98)';
    watermark.style.boxShadow = '0 4px 16px rgba(0, 0, 0, 0.15)';
  };
  watermark.onmouseout = () => {
    watermark.style.transform = 'scale(1)';
    watermark.style.background = 'rgba(255, 255, 255, 0.95)';
    watermark.style.boxShadow = '0 2px 12px rgba(0, 0, 0, 0.1)';
  };
  document.body.appendChild(watermark);
});