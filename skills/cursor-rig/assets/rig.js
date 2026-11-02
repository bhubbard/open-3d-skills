// Cursor-following head rig. The page shows a still image (the plate, or a wider artwork the
// plate sits inside); a <canvas> placed exactly over the head paints one cell from a grid of
// pre-rendered head poses (rows = pitch, up -> down; columns = yaw, image-left -> image-right),
// built by scripts/build_rig.rs.
//
//   <figure class="rig" style="position: relative">
//     <img src="rig/plate.webp" alt="...">            <!-- or your wider hero artwork -->
//     <canvas class="rig-head" style="position: absolute"></canvas>
//   </figure>
//
//   import { loadRig, HeadRig, animate } from "./rig.js";
//   const data = await loadRig("rig/");
//   const head = new HeadRig(document.querySelector(".rig"), data);
//   animate([head]);
//
// If the plate sits inside a wider artwork (an outpainted hero), say where, in artwork pixels:
//   new HeadRig(el, data, { art: { w: 3898, h: 1440, plateX: 1229, plateY: 0 } })

const rand = (a, b) => a + Math.random() * (b - a);
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;

export async function loadRig(base) {
  const rig = await (await fetch(base + "rig.json")).json();
  const imgs = new Array(rig.rows.length).fill(null);
  const load = (i) => new Promise((done) => {
    const im = new Image();
    im.decoding = "async";
    im.onload = () => { imgs[i] = im; done(); };
    im.onerror = done;
    im.src = base + rig.rows[i].src;
  });
  // the level row first so the head can turn left and right at once, then fan out up and down
  const order = rig.pitch.map((_, i) => i).sort((a, b) => Math.abs(rig.pitch[a]) - Math.abs(rig.pitch[b]));
  await load(order[0]);
  (async () => { for (const i of order.slice(1)) await load(i); })();
  return { rig, imgs };
}

export class HeadRig {
  constructor(el, data, { art = null, canvas = null, range = 1 } = {}) {
    this.el = el;
    this.canvas = canvas || el.querySelector("canvas");
    this.ctx = this.canvas.getContext("2d");
    this.rig = data.rig;
    this.imgs = data.imgs;
    this.range = range;                      // 0..1: how far the head may turn
    [this.fw, this.fh] = this.rig.frame;
    const [pw, ph] = this.rig.plate;
    const A = art || { w: pw, h: ph, plateX: 0, plateY: 0 };
    const [bx, by, bw, bh] = this.rig.box;
    Object.assign(this.canvas.style, {
      position: "absolute", pointerEvents: "none",
      left: ((A.plateX + bx) / A.w) * 100 + "%", top: ((A.plateY + by) / A.h) * 100 + "%",
      width: (bw / A.w) * 100 + "%", height: (bh / A.h) * 100 + "%",
    });

    // where the gaze starts from: the eyes if the rig has them, else the box's upper middle
    const level = this.rig.pitch.indexOf(0) >= 0 ? this.rig.pitch.indexOf(0) : 0;
    const e = this.rig.rows[level].cells[this.rig.center].eyes;
    this.hasEyes = !!e;
    this.eyeFx = e ? (e[0][0] + e[1][0]) / 2 / this.fw : 0.5;
    this.eyeFy = e ? (e[0][1] + e[1][1]) / 2 / this.fh : 0.42;

    this.yaw = this.pitch = this.vYaw = this.vPitch = this.tYaw = this.tPitch = 0;
    this.cur = null;
    this.lids = null;                        // null | "both" | "wink"
    this.lidsUntil = 0;
    this.nextBlink = performance.now() + rand(1800, 4200);
    this.visible = true;
    this.dirty = true;

    this.resize();
    new ResizeObserver(() => this.resize()).observe(this.canvas);
    new IntersectionObserver(([en]) => { this.visible = en.isIntersecting; }, { rootMargin: "120px" }).observe(el);
  }

  resize() {
    const r = this.canvas.getBoundingClientRect();
    if (!r.width) return;
    const w = Math.round(Math.min(r.width * Math.min(devicePixelRatio || 1, 2), this.fw * 1.6));
    this.canvas.width = w;
    this.canvas.height = Math.round((w * this.fh) / this.fw);
    this.dirty = true;
  }

  eyes() {
    const r = this.canvas.getBoundingClientRect();
    return { x: r.left + r.width * this.eyeFx, y: r.top + r.height * this.eyeFy, w: r.width };
  }

  // aim at a point on screen; tanh keeps far points from saturating the turn instantly
  lookAt(px, py) {
    const e = this.eyes();
    this.tYaw = Math.tanh((px - e.x) / Math.max(innerWidth * 0.3, e.w * 1.1)) * this.range;
    this.tPitch = Math.tanh((py - e.y) / Math.max(innerHeight * 0.3, e.w * 0.9)) * this.range;
  }

  lookToward(yaw, pitch) { this.tYaw = clamp(yaw, -1, 1); this.tPitch = clamp(pitch, -1, 1); }

  blink(kind = "both", ms = 130) {
    if (!this.hasEyes) return;
    this.lids = kind;
    this.lidsUntil = performance.now() + ms;
    this.dirty = true;
  }

  rowFor(p) {
    const P = this.rig.pitch;
    let best = 0;
    for (let i = 1; i < P.length; i++) if (Math.abs(P[i] - p) < Math.abs(P[best] - p)) best = i;
    if (this.imgs[best]) return best;
    let alt = -1;   // the nearest row that has finished loading
    for (let i = 0; i < P.length; i++) if (this.imgs[i] && (alt < 0 || Math.abs(P[i] - p) < Math.abs(P[alt] - p))) alt = i;
    return alt;
  }

  update(dt, now) {
    // a slightly under-damped spring: the small settle at the end of a turn reads as alive
    const k = 70, c = 2 * (REDUCED ? 1 : 0.78) * Math.sqrt(k);
    this.vYaw += ((this.tYaw - this.yaw) * k - this.vYaw * c) * dt;
    this.vPitch += ((this.tPitch - this.pitch) * k - this.vPitch * c) * dt;
    this.yaw += this.vYaw * dt;
    this.pitch += this.vPitch * dt;

    if (this.hasEyes && !REDUCED && now > this.nextBlink && !this.lids) {
      this.blink("both", rand(110, 150));
      this.nextBlink = now + (Math.random() < 0.18 ? 260 : rand(2400, 5600));   // sometimes a double blink
    }
    if (this.lids && now > this.lidsUntil) { this.lids = null; this.dirty = true; }

    const half = this.rig.center;
    const col = clamp(Math.round(half + clamp(this.yaw, -1, 1) * half), 0, this.rig.cols - 1);
    const row = this.rowFor(clamp(this.pitch, -1, 1));
    if (row < 0) return;
    const key = row * 1000 + col;
    // hard cuts between neighbouring poses: a cross-fade doubles the eyes
    if (!this.cur || this.cur.key !== key) { this.cur = { key, row, col }; this.dirty = true; }
    if (this.dirty) this.draw();
  }

  draw() {
    this.dirty = false;
    const { ctx, canvas, cur } = this;
    if (!cur || !canvas.width) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(this.imgs[cur.row], cur.col * this.fw, 0, this.fw, this.fh, 0, 0, canvas.width, canvas.height);
    if (this.lids) this.drawLids(this.rig.rows[cur.row].cells[cur.col]);
  }

  // closed eyes: cover each eye with its own skin tone, then draw a soft downward lid line
  drawLids(cell) {
    if (!cell.eyes) return;
    const { ctx, canvas } = this;
    const s = canvas.width / this.fw;
    cell.eyes.forEach(([x, y, r], i) => {
      if (this.lids === "wink" && i === 0) return;
      x *= s; y *= s; r *= s;
      const R = r * 1.75, skin = cell.skin[i];
      const g = ctx.createRadialGradient(x, y, R * 0.45, x, y, R);
      g.addColorStop(0, skin);
      g.addColorStop(1, skin + "00");
      ctx.fillStyle = g;
      ctx.beginPath(); ctx.arc(x, y, R, 0, Math.PI * 2); ctx.fill();
      ctx.strokeStyle = "rgba(42, 26, 20, .92)";
      ctx.lineWidth = Math.max(1.2, r * 0.42);
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(x - r * 1.1, y - r * 0.05);
      ctx.quadraticCurveTo(x, y + r * 0.75, x + r * 1.1, y - r * 0.05);
      ctx.stroke();
    });
  }
}

// One loop drives every rig on the page. Follows the pointer while it moves; when nobody is
// around (no pointer for 3.5 s, or a touch device at rest) the head glances about by itself.
export function animate(rigs, { onIdle } = {}) {
  const ptr = { x: innerWidth / 2, y: innerHeight * 0.3, t: -1e9, inside: true };
  const idle = rigs.map(() => ({ next: 0 }));
  addEventListener("pointermove", (e) => { Object.assign(ptr, { x: e.clientX, y: e.clientY, t: performance.now(), inside: true }); }, { passive: true });
  addEventListener("pointerdown", (e) => { Object.assign(ptr, { x: e.clientX, y: e.clientY, t: performance.now() }); }, { passive: true });
  document.documentElement.addEventListener("pointerleave", () => { ptr.inside = false; });

  let last = performance.now();
  const tick = (now) => {
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    const active = ptr.inside && now - ptr.t < 3500;
    rigs.forEach((r, i) => {
      if (!r.visible) return;
      if (active) r.lookAt(ptr.x, ptr.y);
      else if (!REDUCED && now > idle[i].next) {
        if (Math.random() < 0.35) r.lookToward(0, rand(-0.15, 0.05));
        else r.lookToward(rand(-0.75, 0.75), rand(-0.45, 0.3));
        idle[i].next = now + rand(1400, 3200);
        onIdle && onIdle(r);
      } else if (REDUCED && !active) r.lookToward(0, 0);
      r.update(dt, now);
    });
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  return ptr;
}
