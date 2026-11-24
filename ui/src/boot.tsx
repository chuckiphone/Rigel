import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/**
 * The opening frame.
 *
 * A dust cloud drifts, spirals into a core, compresses once, and bursts into the orange star from
 * `mark.tsx`. It is one canvas, drawn at the window's pixel ratio, with no image or animation
 * dependency. The intro is skippable, gives reduced-motion users a still star, and stays out of
 * the way of automation; `?boot=1` forces it for a visual check.
 */

const COLLAPSE_AT = 700;
const COLLAPSE_END = 1_460;
const COMPRESS_AT = 1_520;
const BURST_AT = 1_810;
const FADE_AT = 2_940;
const FADE_MS = 560;

type Palette = {
  void: string;
  dustHot: string;
  dustMid: string;
  dustEdge: string;
  rustHot: string;
  rustMid: string;
  rustEdge: string;
  blueHot: string;
  blueMid: string;
  blueEdge: string;
  lane: string;
  laneEdge: string;
  glow: string;
  glowMid: string;
  glowEdge: string;
  core: string;
  star: string;
  starCore: string;
  spark: string;
  flash: string;
  ring: string;
  ray: string;
  rayEdge: string;
  skip: string;
};

type Speck = {
  x: number;
  y: number;
  size: number;
  alpha: number;
  phase: number;
  color: string;
};

type Mote = {
  angle: number;
  radius: number;
  drift: number;
  size: number;
  phase: number;
  color: string;
};

type Spark = {
  angle: number;
  speed: number;
  life: number;
  width: number;
  color: string;
};

function bootEnabled(): boolean {
  const request = new URLSearchParams(window.location.search).get("boot");
  if (request === "1") return true;
  if (request === "0") return false;
  return !navigator.webdriver;
}

function bootPalette(): Palette {
  const styles = getComputedStyle(document.documentElement);
  const token = (name: string) => styles.getPropertyValue(name).trim() || "transparent";
  return {
    void: token("--boot-void"),
    dustHot: token("--boot-dust-hot"),
    dustMid: token("--boot-dust-mid"),
    dustEdge: token("--boot-dust-edge"),
    rustHot: token("--boot-rust-hot"),
    rustMid: token("--boot-rust-mid"),
    rustEdge: token("--boot-rust-edge"),
    blueHot: token("--boot-blue-hot"),
    blueMid: token("--boot-blue-mid"),
    blueEdge: token("--boot-blue-edge"),
    lane: token("--boot-lane"),
    laneEdge: token("--boot-lane-edge"),
    glow: token("--boot-glow"),
    glowMid: token("--boot-glow-mid"),
    glowEdge: token("--boot-glow-edge"),
    core: token("--boot-core"),
    star: token("--boot-star"),
    starCore: token("--boot-star-core"),
    spark: token("--boot-spark"),
    flash: token("--boot-flash"),
    ring: token("--boot-ring"),
    ray: token("--boot-ray"),
    rayEdge: token("--boot-ray-edge"),
    skip: token("--boot-skip"),
  };
}

function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state * 1_664_525 + 1_013_904_223) >>> 0;
    return state / 4_294_967_296;
  };
}

function clamp(value: number, low = 0, high = 1): number {
  return Math.min(high, Math.max(low, value));
}

function smoothstep(from: number, to: number, value: number): number {
  const p = clamp((value - from) / (to - from));
  return p * p * (3 - 2 * p);
}

function easeInOutCubic(value: number): number {
  const p = clamp(value);
  return p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2;
}

function easeOutBack(value: number): number {
  const p = clamp(value);
  const c1 = 1.70158;
  const c3 = c1 + 1;
  return 1 + c3 * Math.pow(p - 1, 3) + c1 * Math.pow(p - 1, 2);
}

function radial(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  radius: number,
  stops: [number, string][],
): CanvasGradient {
  const gradient = ctx.createRadialGradient(x, y, 0, x, y, radius);
  for (const [at, color] of stops) gradient.addColorStop(at, color);
  return gradient;
}

function blob(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  radius: number,
  hot: string,
  mid: string,
  edge: string,
  alpha: number,
): void {
  const gradient = ctx.createRadialGradient(x, y, 0, x, y, radius);
  gradient.addColorStop(0, hot);
  gradient.addColorStop(0.38, mid);
  gradient.addColorStop(1, edge);
  ctx.globalAlpha = alpha;
  ctx.fillStyle = gradient;
  ctx.beginPath();
  ctx.arc(x, y, radius, 0, Math.PI * 2);
  ctx.fill();
}

function nebulaTexture(p: Palette): HTMLCanvasElement {
  const size = 1_024;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) return canvas;
  const random = seeded(0x51a7e);
  ctx.globalCompositeOperation = "lighter";

  // The warm heart every arm grows from, with one cool and one rust shoulder for depth.
  blob(ctx, size / 2, size / 2, size * 0.34, p.dustHot, p.dustMid, p.dustEdge, 0.9);
  blob(ctx, size * 0.64, size * 0.37, size * 0.29, p.blueHot, p.blueMid, p.blueEdge, 0.72);
  blob(ctx, size * 0.34, size * 0.62, size * 0.25, p.rustHot, p.rustMid, p.rustEdge, 0.66);
  blob(ctx, size / 2, size / 2, size * 0.16, p.core, p.dustHot, p.dustEdge, 0.22);

  for (let index = 0; index < 128; index += 1) {
    const arm = index % 3;
    const spread = Math.pow(random(), 0.64);
    const radius = size * (0.025 + spread * 0.455);
    const angle = arm * ((Math.PI * 2) / 3) + spread * 2.65 + (random() - 0.5) * 0.68;
    const x = size / 2 + Math.cos(angle) * radius;
    const y = size / 2 + Math.sin(angle) * radius * 0.7 + (random() - 0.5) * size * 0.12;
    const blobRadius = size * (0.014 + Math.pow(random(), 1.72) * 0.125);
    const roll = random();
    const warm = roll > 0.82 ? [p.blueHot, p.blueMid, p.blueEdge] : roll > 0.64
      ? [p.rustHot, p.rustMid, p.rustEdge]
      : [p.dustHot, p.dustMid, p.dustEdge];
    blob(ctx, x, y, blobRadius, warm[0], warm[1], warm[2], 0.42 + random() * 0.58);
  }

  // A few long, low-alpha filaments make the cloud read as gas rather than a soft blur.
  ctx.save();
  ctx.translate(size / 2, size / 2);
  ctx.scale(1, 0.7);
  ctx.lineCap = "round";
  for (let index = 0; index < 34; index += 1) {
    const start = random() * Math.PI * 2;
    const radius = size * (0.07 + random() * 0.37);
    const sweep = 0.12 + random() * 0.62;
    ctx.globalAlpha = 0.1 + random() * 0.26;
    ctx.strokeStyle = index % 5 === 0 ? p.blueMid : p.dustMid;
    ctx.lineWidth = 0.8 + random() * 2.8;
    ctx.beginPath();
    ctx.arc(0, 0, radius, start, start + sweep);
    ctx.stroke();
  }
  ctx.restore();

  // Carve a few dark lanes so the cloud has depth instead of one even glow.
  ctx.globalCompositeOperation = "destination-out";
  for (let index = 0; index < 30; index += 1) {
    const angle = random() * Math.PI * 2;
    const radius = size * (0.04 + random() * 0.34);
    const x = size / 2 + Math.cos(angle) * radius;
    const y = size / 2 + Math.sin(angle) * radius * 0.72;
    const lane = size * (0.035 + random() * 0.09);
    const gradient = radial(ctx, x, y, lane, [
      [0, p.lane],
      [1, p.laneEdge],
    ]);
    ctx.globalAlpha = 0.3 + random() * 0.45;
    ctx.fillStyle = gradient;
    ctx.beginPath();
    ctx.arc(x, y, lane, 0, Math.PI * 2);
    ctx.fill();
  }

  // A handful of hot knots keep the cloud from looking like a soft gradient.
  ctx.globalCompositeOperation = "lighter";
  for (let index = 0; index < 24; index += 1) {
    const angle = random() * Math.PI * 2;
    const radius = size * (0.03 + Math.pow(random(), 0.7) * 0.4);
    const x = size / 2 + Math.cos(angle) * radius;
    const y = size / 2 + Math.sin(angle) * radius * 0.7;
    const dot = 1 + random() * 2.6;
    ctx.globalAlpha = 0.2 + random() * 0.5;
    ctx.fillStyle = random() > 0.8 ? p.blueHot : p.starCore;
    ctx.beginPath();
    ctx.arc(x, y, dot, 0, Math.PI * 2);
    ctx.fill();
  }

  ctx.globalAlpha = 1;
  return canvas;
}

function makeSpecks(p: Palette): Speck[] {
  const random = seeded(0x9e3779b9);
  return Array.from({ length: 240 }, () => {
    const roll = random();
    return {
      x: random(),
      y: random(),
      size: 0.45 + Math.pow(random(), 2.4) * 1.8,
      alpha: 0.18 + random() * 0.72,
      phase: 0.4 + random() * 2.4,
      color: roll > 0.86 ? p.blueHot : roll > 0.68 ? p.dustHot : p.starCore,
    };
  });
}

function makeMotes(p: Palette): Mote[] {
  const random = seeded(0x8badf00d);
  return Array.from({ length: 150 }, () => {
    const roll = random();
    return {
      angle: random() * Math.PI * 2,
      radius: 0.12 + Math.pow(random(), 0.7) * 0.52,
      drift: 0.02 + random() * 0.12,
      size: 0.8 + Math.pow(random(), 2) * 2.4,
      phase: random() * Math.PI * 2,
      color: roll > 0.88 ? p.blueHot : roll > 0.66 ? p.rustHot : p.dustHot,
    };
  });
}

function makeSparks(p: Palette): Spark[] {
  const random = seeded(0x1a2b3c4d);
  return Array.from({ length: 190 }, () => {
    const roll = random();
    return {
      angle: random() * Math.PI * 2,
      speed: 0.16 + Math.pow(random(), 1.6) * 0.9,
      life: 480 + random() * 760,
      width: 0.5 + random() * 1.8,
      color: roll > 0.88 ? p.starCore : roll > 0.72 ? p.spark : p.dustHot,
    };
  });
}

function traceStar(ctx: CanvasRenderingContext2D, cx: number, cy: number, radius: number): void {
  const pinch = radius * 0.1125;
  ctx.beginPath();
  ctx.moveTo(cx, cy - radius);
  ctx.quadraticCurveTo(cx + pinch, cy - pinch, cx + radius, cy);
  ctx.quadraticCurveTo(cx + pinch, cy + pinch, cx, cy + radius);
  ctx.quadraticCurveTo(cx - pinch, cy + pinch, cx - radius, cy);
  ctx.quadraticCurveTo(cx - pinch, cy - pinch, cx, cy - radius);
  ctx.closePath();
}

function beam(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  length: number,
  width: number,
  angle: number,
  alpha: number,
  p: Palette,
): void {
  ctx.save();
  ctx.translate(cx, cy);
  ctx.rotate(angle);
  const gradient = ctx.createLinearGradient(-length, 0, length, 0);
  gradient.addColorStop(0, p.rayEdge);
  gradient.addColorStop(0.5, p.ray);
  gradient.addColorStop(1, p.rayEdge);
  ctx.globalAlpha = alpha;
  ctx.fillStyle = gradient;
  ctx.fillRect(-length, -width / 2, length * 2, width);
  ctx.restore();
}

function paint(
  ctx: CanvasRenderingContext2D,
  t: number,
  width: number,
  height: number,
  p: Palette,
  nebula: HTMLCanvasElement,
  specks: Speck[],
  motes: Mote[],
  sparks: Spark[],
): void {
  const min = Math.min(width, height);
  const cx = width / 2;
  const cy = height / 2;
  ctx.globalAlpha = 1;
  ctx.globalCompositeOperation = "source-over";
  ctx.clearRect(0, 0, width, height);
  ctx.fillStyle = p.void;
  ctx.fillRect(0, 0, width, height);

  // The star field is the fixed room; only the dust and the core move.
  ctx.globalCompositeOperation = "lighter";
  for (const speck of specks) {
    const twinkle = 0.45 + 0.55 * Math.abs(Math.sin(t * 0.0011 * speck.phase + speck.phase * 7));
    const grow = smoothstep(1_700, 2_100, t) * 0.28;
    ctx.globalAlpha = speck.alpha * twinkle;
    ctx.fillStyle = speck.color;
    const x = speck.x * width;
    const y = speck.y * height;
    const size = speck.size * (1 + grow);
    ctx.fillRect(x, y, size, size);
  }

  const collapse = smoothstep(COLLAPSE_AT, COLLAPSE_END, t);
  const burstOut = 1 - smoothstep(BURST_AT, BURST_AT + 320, t);
  const intro = smoothstep(0, 500, t);
  const cloudScale = 1.045 - 0.94 * easeInOutCubic(collapse);
  const cloudAlpha = intro * (0.58 + 0.42 * collapse) * burstOut;
  if (cloudAlpha > 0.005) {
    const cloudSize = min * 1.82;
    const spin = 0.000_028 * t + 1.18 * easeInOutCubic(collapse);
    ctx.save();
    ctx.translate(cx, cy);
    ctx.rotate(spin);
    ctx.scale(cloudScale, cloudScale * 0.93);
    ctx.globalAlpha = cloudAlpha * 0.3;
    ctx.drawImage(nebula, -cloudSize / 2, -cloudSize / 2, cloudSize, cloudSize);
    ctx.rotate(-0.09);
    ctx.scale(1.06, 1.06);
    ctx.globalAlpha = cloudAlpha * 0.62;
    ctx.drawImage(nebula, -cloudSize / 2, -cloudSize / 2, cloudSize, cloudSize);
    ctx.restore();
  }

  // Grains of dust follow the same collapse as the cloud, but add the spiral.
  if (t < BURST_AT + 500) {
    const pull = smoothstep(COLLAPSE_AT - 80, COLLAPSE_END, t);
    const moteAlpha = (0.25 + 0.62 * pull) * burstOut;
    ctx.globalCompositeOperation = "lighter";
    ctx.lineCap = "round";
    for (const mote of motes) {
      const radius = min * mote.radius * (1 - 0.93 * easeInOutCubic(pull));
      const angle = mote.angle + t * mote.drift * 0.000_12 + pull * 2.35;
      const x = cx + Math.cos(angle) * radius;
      const y = cy + Math.sin(angle) * radius * 0.74;
      const tail = min * (0.002 + 0.012 * pull) * (0.7 + 0.3 * Math.sin(mote.phase));
      const px = cx + Math.cos(angle - 0.24) * (radius + tail);
      const py = cy + Math.sin(angle - 0.24) * (radius + tail) * 0.74;
      ctx.globalAlpha = moteAlpha * (0.25 + 0.55 * Math.abs(Math.sin(mote.phase + t * 0.0008)));
      ctx.strokeStyle = mote.color;
      ctx.lineWidth = mote.size * (0.65 + pull * 0.55);
      ctx.beginPath();
      ctx.moveTo(px, py);
      ctx.lineTo(x, y);
      ctx.stroke();
    }
  }

  // The core is a hot sphere before it is a star. It pinches in just before the burst.
  const coreIn = smoothstep(440, 1_500, t);
  const pinch = t > COMPRESS_AT && t < BURST_AT
    ? 1 - 0.14 * Math.sin(Math.PI * clamp((t - COMPRESS_AT) / (BURST_AT - COMPRESS_AT)))
    : 1;
  const coreFade = 1 - smoothstep(BURST_AT, BURST_AT + 260, t);
  if (coreIn > 0.01 && coreFade > 0.01) {
    const radius = min * (0.02 + 0.062 * coreIn) * (0.78 + 0.22 * easeInOutCubic(collapse)) * pinch;
    ctx.globalCompositeOperation = "lighter";
    ctx.globalAlpha = coreIn * coreFade * 0.38;
    ctx.fillStyle = radial(ctx, cx, cy, radius * 8, [
      [0, p.glow],
      [0.45, p.glowMid],
      [1, p.glowEdge],
    ]);
    ctx.beginPath();
    ctx.arc(cx, cy, radius * 8, 0, Math.PI * 2);
    ctx.fill();

    const ball = ctx.createRadialGradient(
      cx - radius * 0.3,
      cy - radius * 0.34,
      radius * 0.05,
      cx,
      cy,
      radius,
    );
    ball.addColorStop(0, p.starCore);
    ball.addColorStop(0.36, p.star);
    ball.addColorStop(1, p.rustHot);
    ctx.globalAlpha = coreIn * coreFade;
    ctx.fillStyle = ball;
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();

    ctx.globalAlpha = coreIn * coreFade * 0.42;
    ctx.strokeStyle = p.starCore;
    ctx.lineWidth = Math.max(0.6, radius * 0.035);
    ctx.beginPath();
    ctx.arc(cx, cy, radius * 0.86, -2.5, -1.02);
    ctx.stroke();

    // Rotating plasma bands keep the held ball alive without turning it into a ringed planet.
    ctx.save();
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.clip();
    for (let band = 0; band < 3; band += 1) {
      ctx.save();
      ctx.translate(cx, cy);
      ctx.rotate(t * 0.0011 * (band % 2 === 0 ? 1 : -1) + band * 2.14);
      ctx.globalAlpha = coreIn * coreFade * (0.14 - band * 0.022);
      ctx.strokeStyle = p.starCore;
      ctx.lineWidth = radius * (0.045 - band * 0.008);
      ctx.beginPath();
      ctx.arc(0, 0, radius * (0.3 + band * 0.22), 0.18, 1.9);
      ctx.stroke();
      ctx.restore();
    }
    ctx.restore();

    // The compression is readable for one held beat: a ring closes onto the ball.
    if (t > COMPRESS_AT - 80 && t < BURST_AT) {
      const compression = clamp((t - (COMPRESS_AT - 80)) / (BURST_AT - (COMPRESS_AT - 80)));
      ctx.globalAlpha = (1 - compression) * 0.32 + 0.04;
      ctx.strokeStyle = p.ring;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(cx, cy, radius * (2.8 - 1.2 * compression), 0, Math.PI * 2);
      ctx.stroke();
    }
  }

  const age = t - BURST_AT;
  if (age >= 0) {
    // Flash first, then a pair of shockwaves, then sparks.
    const flash = 1 - smoothstep(0, 280, age);
    if (flash > 0.01) {
      const flashRadius = min * 0.82;
      ctx.globalCompositeOperation = "lighter";
      ctx.globalAlpha = flash;
      ctx.fillStyle = radial(ctx, cx, cy, flashRadius, [
        [0, p.flash],
        [0.34, p.glow],
        [1, p.glowEdge],
      ]);
      ctx.beginPath();
      ctx.arc(cx, cy, flashRadius, 0, Math.PI * 2);
      ctx.fill();
    }

    ctx.globalCompositeOperation = "lighter";
    for (let ring = 0; ring < 2; ring += 1) {
      const ringAge = age - ring * 80;
      if (ringAge < 0 || ringAge > 780) continue;
      const progress = clamp(ringAge / 780);
      const radius = min * (0.03 + (1 - Math.pow(1 - progress, 2)) * 0.7);
      ctx.globalAlpha = Math.pow(1 - progress, 2.4) * (0.62 - ring * 0.16);
      ctx.strokeStyle = p.ring;
      ctx.lineWidth = 1.1 + (1 - progress) * 2.2;
      ctx.beginPath();
      ctx.arc(cx, cy, radius, 0, Math.PI * 2);
      ctx.stroke();
    }

    ctx.lineCap = "round";
    for (const spark of sparks) {
      if (age > spark.life) continue;
      const progress = clamp(age / spark.life);
      const distance = min * spark.speed * (1 - Math.pow(1 - progress, 2));
      const tailDistance = distance * 0.92;
      const x = cx + Math.cos(spark.angle) * distance;
      const y = cy + Math.sin(spark.angle) * distance;
      const tx = cx + Math.cos(spark.angle) * tailDistance;
      const ty = cy + Math.sin(spark.angle) * tailDistance;
      ctx.globalAlpha = Math.pow(1 - progress, 1.45) * 0.9;
      ctx.strokeStyle = spark.color;
      ctx.lineWidth = spark.width * (1 - progress * 0.55);
      ctx.beginPath();
      ctx.moveTo(tx, ty);
      ctx.lineTo(x, y);
      ctx.stroke();
    }
  }

  // The burst resolves into the same four-point star as the mark, with no wordmark.
  const reveal = smoothstep(BURST_AT + 10, BURST_AT + 380, t);
  if (reveal > 0.001) {
    const starRadius = Math.max(60, Math.min(124, min * 0.122));
    const settle = easeOutBack(reveal);
    const breath = 1 + 0.004 * Math.sin(Math.max(0, age) / 220);
    const radius = starRadius * settle * breath;
    const starAlpha = smoothstep(0, 0.25, reveal);
    const haloPulse = 1 + 0.018 * Math.sin(Math.max(0, age) / 260);

    ctx.globalCompositeOperation = "lighter";
    ctx.globalAlpha = starAlpha * 0.62;
    ctx.fillStyle = radial(ctx, cx, cy, radius * 4.6 * haloPulse, [
      [0, p.glow],
      [0.4, p.glowMid],
      [1, p.glowEdge],
    ]);
    ctx.beginPath();
    ctx.arc(cx, cy, radius * 4.6 * haloPulse, 0, Math.PI * 2);
    ctx.fill();

    const rayFade = Math.exp(-Math.max(0, age - 240) / 950);
    beam(ctx, cx, cy, radius * (2.1 + 2.0 * rayFade), 1.3 + 1.6 * rayFade, 0, starAlpha * 0.7, p);
    beam(
      ctx,
      cx,
      cy,
      radius * (1.4 + 1.1 * rayFade),
      0.9 + 1.0 * rayFade,
      Math.PI / 2,
      starAlpha * 0.5,
      p,
    );

    // The mark's faint disc sits behind the star, never around it as a ring.
    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = starAlpha * 0.14;
    ctx.fillStyle = p.star;
    ctx.beginPath();
    ctx.arc(cx, cy, radius * 0.6875, 0, Math.PI * 2);
    ctx.fill();

    ctx.globalAlpha = starAlpha;
    ctx.fillStyle = p.star;
    traceStar(ctx, cx, cy, radius);
    ctx.fill();

    ctx.globalAlpha = starAlpha * 0.72;
    ctx.fillStyle = p.starCore;
    ctx.beginPath();
    ctx.arc(cx, cy, Math.max(1.2, radius * 0.09), 0, Math.PI * 2);
    ctx.fill();

    // A small constellation around the held mark keeps the frame alive without moving the logo.
    for (let index = 0; index < 14; index += 1) {
      const angle = index * 2.399 + age * 0.000_18;
      const orbit = radius * (1.36 + 0.55 * Math.abs(Math.sin(index * 1.71)));
      const twinkle = 0.3 + 0.7 * Math.abs(Math.sin(age * 0.0017 + index * 2.1));
      ctx.globalAlpha = starAlpha * twinkle * 0.55;
      ctx.fillStyle = index % 5 === 0 ? p.starCore : p.spark;
      ctx.beginPath();
      ctx.arc(cx + Math.cos(angle) * orbit, cy + Math.sin(angle) * orbit, 1.1, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  ctx.globalAlpha = 1;
  ctx.globalCompositeOperation = "source-over";
}

export default function BootSequence() {
  const [enabled] = useState(bootEnabled);
  const [leaving, setLeaving] = useState(false);
  const [done, setDone] = useState(false);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  const skip = useCallback(() => setLeaving(true), []);

  useEffect(() => {
    if (!enabled) return;
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const timer = window.setTimeout(() => setLeaving(true), reduced ? 700 : FADE_AT);
    return () => window.clearTimeout(timer);
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setLeaving(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled]);

  useEffect(() => {
    if (!enabled || !leaving) return;
    const timer = window.setTimeout(() => setDone(true), FADE_MS);
    return () => window.clearTimeout(timer);
  }, [enabled, leaving]);

  useEffect(() => {
    if (!enabled) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const palette = bootPalette();
    const nebula = nebulaTexture(palette);
    const specks = makeSpecks(palette);
    const motes = makeMotes(palette);
    const sparks = makeSparks(palette);
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    let frame = 0;
    let width = 0;
    let height = 0;

    const resize = () => {
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      width = window.innerWidth;
      height = window.innerHeight;
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      if (reduced) paint(ctx, FADE_AT + 60, width, height, palette, nebula, specks, motes, sparks);
    };

    resize();
    window.addEventListener("resize", resize);

    if (!reduced) {
      const start = performance.now();
      const tick = (now: number) => {
        paint(
          ctx,
          now - start,
          width,
          height,
          palette,
          nebula,
          specks,
          motes,
          sparks,
        );
        frame = requestAnimationFrame(tick);
      };
      frame = requestAnimationFrame(tick);
    }

    return () => {
      window.removeEventListener("resize", resize);
      cancelAnimationFrame(frame);
    };
  }, [enabled]);

  if (!enabled || done) return null;

  return createPortal(
    <div className={`boot${leaving ? " leaving" : ""}`} role="group" aria-label="Rigel is starting">
      <canvas ref={canvasRef} className="boot-canvas" aria-hidden="true" />
      <button type="button" className="boot-skip" onClick={skip}>
        Skip intro
      </button>
    </div>,
    document.body,
  );
}
