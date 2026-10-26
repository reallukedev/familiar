import { useId, useLayoutEffect, useRef, type PointerEvent } from 'react';
import { useReducedMotion } from '../lib/platform';
import styles from './Familiar.module.css';

/**
 * Familiar, the mascot: the app icon's Mac with cat ears, alive. One inline
 * SVG built from the icon's geometry, with the ears, eyes, eyelids, mouth,
 * bezel and screen as separate parts. The motion spec is at the top of Familiar.module.css.
 */
export type FamiliarState = 'idle' | 'thinking' | 'happy' | 'attention' | 'sleeping' | 'waking' | 'sad' | 'offline';

export const FAMILIAR_STATES: FamiliarState[] = ['idle', 'thinking', 'happy', 'attention', 'sleeping', 'waking', 'sad', 'offline'];

type Props = {
  state?: FamiliarState;
  /** Rendered width and height in CSS pixels; below 44 it switches to the small icon's bolder geometry. */
  size?: number;
  /** icon: on its graphite squircle, like the app icon. glyph: the character alone, for dark surfaces. */
  variant?: 'icon' | 'glyph';
  /** Names the image for assistive technology; decorative when left out. */
  label?: string;
  className?: string;
};

// The icon's character, in its own 1024 grid. The regular geometry is icon.svg;
// the compact one is icon-small.svg (thicker bezel, bigger eyes, no stand),
// which stays legible at 20 to 40 px.
type Geometry = {
  screen: { x: number; y: number; w: number; h: number; r: number };
  eye: { rx: number; ry: number };
  stand: boolean;
  feet: number;
  arc: number;
  /** How icon.svg places the character on its squircle: x' = k·x + ox. */
  place: { k: number; ox: number; oy: number };
  glyph: { x: number; y: number; s: number };
};

const REGULAR: Geometry = {
  screen: { x: 274, y: 360, w: 476, h: 308, r: 46 },
  eye: { rx: 34, ry: 48 },
  stand: true,
  feet: 822,
  arc: 15,
  place: { k: 1.06, ox: -30.7, oy: -38.7 },
  glyph: { x: 182, y: 189, s: 660 },
};
const COMPACT: Geometry = {
  screen: { x: 296, y: 382, w: 432, h: 264, r: 24 },
  eye: { rx: 48, ry: 64 },
  stand: false,
  feet: 710,
  arc: 26,
  place: { k: 1.18, ox: -92.2, oy: -52.2 },
  glyph: { x: 212, y: 163, s: 600 },
};

const EYES = { L: 420, R: 604, y: 514 };
// The face's offsets when it has a mouth: eyes up, and the nose a clear step below them.
const FACE_RISE = -26;
const MOUTH_DROP = 12;
// Ears pivot where they meet the bezel, so they swing from the base.
const EAR_PIVOT = { L: 352, R: 672, y: 322 };
const HEAD_PIVOT = '512px 710px';
// The lid colour is the screen gradient where the eyes sit, so it disappears into it.
const LID = '#1d1d26';

const HEAD = { x: 232, y: 318, w: 560, h: 392, r: 80 };
// A neck that flares into the foot, like the Studio Display's.
const STAND = 'M458 706h108l20 84q2 8-6 8H444q-8 0-6-8Z';
const BASE = { x: 386, y: 794, w: 252, h: 26 };
// Rounded tips with a slightly convex outer edge; the bases hide behind the head.
const EAR_PATH = {
  L: 'M258 420C262 342 274 272 292 228C300 207 318 201 334 214C376 248 416 290 446 340Z',
  R: 'M766 420C762 342 750 272 732 228C724 207 706 201 690 214C648 248 608 290 578 340Z',
};
const INNER_EAR_PATH = {
  L: 'M300 360C302 314 308 276 318 252C322 242 330 240 338 246C364 268 390 296 408 330Z',
  R: 'M724 360C722 314 716 276 706 252C702 242 694 240 686 246C660 268 634 296 616 330Z',
};

// The mouth sits under a small nose, in the same light as the eyes. Each
// expression is its own shape; a state change shrinks one away as the next grows in.
const MOUTH_ORIGIN = '512px 596px';
const NOSE = 'M505 571h14q4.5 0 1.8 3.6l-5.6 6.6q-2.2 2.4-4.4 0l-5.6-6.6Q500.5 571 505 571Z';
type MouthName = 'cat' | 'grin' | 'hmm' | 'o' | 'snooze' | 'frown' | 'flat' | 'blep' | 'yawn';
const MOUTHS: MouthName[] = ['cat', 'grin', 'hmm', 'o', 'snooze', 'frown', 'flat', 'blep', 'yawn'];
const MOUTH: Record<MouthName, { stroke?: string; fill?: string }> = {
  cat: { stroke: 'M512 582v6M512 588q-12 18-26 4M512 588q12 18 26 4' },
  grin: { stroke: 'M512 582v6M512 588q-17 24-34 0M512 588q17 24 34 0', fill: 'M492 602c6 30 34 30 40 0-8 6-15 4-20-4-5 8-12 10-20 4Z' },
  hmm: { stroke: 'M494 602q10 6 20 0t22-6' },
  o: { stroke: 'M512 589c8.5 0 13 7 13 15.5S520.5 620 512 620s-13-7-13-15.5 4.5-15.5 13-15.5Z' },
  snooze: { stroke: 'M512 582v6M512 588q-9 12-18 6M512 588q9 12 18 6' },
  frown: { stroke: 'M488 612q12-16 24-12 12-4 24 12' },
  flat: { stroke: 'M497 602h30' },
  // The tip of a tongue, left out for a moment: the idle cat's rare blep.
  blep: { fill: 'M505 597h14v5a7 7 0 0 1-14 0Z' },
  // Waking only: wide open, lit from inside.
  yawn: { stroke: 'M512 586c13 0 20 9.5 20 21.5S525 630 512 630s-20-10.5-20-22.5 7-21.5 20-21.5Z', fill: 'M512 586c13 0 20 9.5 20 21.5S525 630 512 630s-20-10.5-20-22.5 7-21.5 20-21.5Z' },
};

/**
 * A rounded rectangle with Apple's continuous corners (the iOS 7 icon mask
 * curve): each corner starts 1.53r from the vertex and eases into the arc, so
 * there is no kink where a straight edge meets the curve.
 */
export function squircle(x: number, y: number, w: number, h: number, r: number) {
  r = Math.min(r, w / 2 / 1.5287, h / 2 / 1.5287);
  const R = x + w;
  const B = y + h;
  // Vertex, the direction of travel into it and out of it, clockwise from the top edge.
  const corners: [number, number, number, number, number, number][] = [
    [R, y, 1, 0, 0, 1],
    [R, B, 0, 1, -1, 0],
    [x, B, -1, 0, 0, -1],
    [x, y, 0, -1, 1, 0],
  ];
  const n = (v: number) => Math.round(v * 100) / 100;
  let d = '';
  for (const [vx, vy, ix, iy, ox, oy] of corners) {
    const pt = (a: number, b: number) => `${n(vx - a * r * ix + b * r * ox)} ${n(vy - a * r * iy + b * r * oy)}`;
    d += `${d ? 'L' : 'M'}${pt(1.52866483, 0)}`;
    d += `C${pt(1.08849323, 0)} ${pt(0.86840689, 0.02208862)} ${pt(0.66993427, 0.065496)}`;
    d += `L${pt(0.63149399, 0.074911)}`;
    d += `C${pt(0.37282392, 0.16905899)} ${pt(0.16905899, 0.37282392)} ${pt(0.074911, 0.63149399)}`;
    d += `L${pt(0.065496, 0.66993427)}`;
    d += `C${pt(0.02208862, 0.86840689)} ${pt(0, 1.08849323)} ${pt(0, 1.52866471)}`;
  }
  return `${d}Z`;
}

export function Familiar({ state = 'idle', size = 56, variant = 'icon', label, className = '' }: Props) {
  const ref = useRef<SVGSVGElement>(null);
  const director = useRef<Director | null>(null);
  const reduced = useReducedMotion();
  const compact = size < 44;
  const geo = compact ? COMPACT : REGULAR;
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, '');

  useLayoutEffect(() => {
    const svg = ref.current;
    if (!svg) return;
    const instance = new Director(svg);
    director.current = instance;
    return () => {
      instance.destroy();
      director.current = null;
    };
  }, [compact]);

  useLayoutEffect(() => {
    director.current?.set(state, geo, reduced);
  }, [state, geo, reduced]);

  const { k, ox, oy } = geo.place;
  const box = variant === 'icon' ? { x: -ox / k, y: -oy / k, s: 1024 / k } : geo.glyph;
  const s = geo.screen;
  const pointer = reduced
    ? {}
    : {
        onPointerDown: (event: PointerEvent) => {
          if (event.pointerType !== 'mouse' || event.button === 0) director.current?.press();
        },
        onPointerUp: () => director.current?.release(),
        onPointerCancel: () => director.current?.release(),
        onPointerLeave: () => director.current?.release(),
      };

  return (
    <svg
      ref={ref}
      className={`${styles.root} ${className}`}
      data-variant={variant}
      width={size}
      height={size}
      viewBox={`0 0 ${box.s} ${box.s}`}
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
      {...pointer}
    >
      <defs>
        <linearGradient id={`${id}bg`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#474753" />
          <stop offset="1" stopColor="#121217" />
        </linearGradient>
        {/* Light from above, falling off across the squircle. */}
        <radialGradient id={`${id}sky`} cx="0.5" cy="0" r="0.85">
          <stop offset="0" stopColor="#fff" stopOpacity="0.16" />
          <stop offset="1" stopColor="#fff" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${id}shadow`} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stopColor="#000" stopOpacity="0.5" />
          <stop offset="0.6" stopColor="#000" stopOpacity="0.2" />
          <stop offset="1" stopColor="#000" stopOpacity="0" />
        </radialGradient>
        {/* White enamel, lit from above: bright top, a soft turn to grey at the base. */}
        <linearGradient id={`${id}face`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#ffffff" />
          <stop offset="0.55" stopColor="#f3f3f6" />
          <stop offset="1" stopColor="#d6d6de" />
        </linearGradient>
        <linearGradient id={`${id}innerEar`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#f1d2d9" />
          <stop offset="1" stopColor="#e2b7c2" />
        </linearGradient>
        {/* The screen sits in a recess: shaded where the bezel overhangs it, lit along the bottom. */}
        <linearGradient id={`${id}recess`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#b4b4be" />
          <stop offset="1" stopColor="#f4f4f7" />
        </linearGradient>
        <linearGradient id={`${id}screen`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#2a2a35" />
          <stop offset="1" stopColor="#101016" />
        </linearGradient>
        <linearGradient id={`${id}inset`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#000" stopOpacity="0.45" />
          <stop offset="0.14" stopColor="#000" stopOpacity="0" />
        </linearGradient>
        {/* A faint sheen across the glass, cut on a diagonal. */}
        <linearGradient id={`${id}glass`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#fff" stopOpacity="0.09" />
          <stop offset="0.42" stopColor="#fff" stopOpacity="0.025" />
          <stop offset="0.42" stopColor="#fff" stopOpacity="0" />
        </linearGradient>
        <linearGradient id={`${id}eye`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fff2c2" />
          <stop offset="0.45" stopColor="#ffd04d" />
          <stop offset="1" stopColor="#ffa90f" />
        </linearGradient>
        <radialGradient id={`${id}eyeGlow`} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stopColor="#ffb21f" stopOpacity="0.4" />
          <stop offset="1" stopColor="#ffb21f" stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${id}glow`} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stopColor="#ffc93d" stopOpacity="0.3" />
          <stop offset="1" stopColor="#ffc93d" stopOpacity="0" />
        </radialGradient>
        {/* Brushed aluminium, turned: dark at the sides, a bright line down the middle. */}
        <linearGradient id={`${id}metal`} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor="#a4a4ae" />
          <stop offset="0.38" stopColor="#e6e6eb" />
          <stop offset="0.5" stopColor="#fafafc" />
          <stop offset="0.66" stopColor="#d4d4db" />
          <stop offset="1" stopColor="#9e9ea8" />
        </linearGradient>
        <linearGradient id={`${id}foot`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fbfbfd" />
          <stop offset="1" stopColor="#bdbdc6" />
        </linearGradient>
        <clipPath id={`${id}screenClip`}>
          <path d={squircle(s.x, s.y, s.w, s.h, s.r)} />
        </clipPath>
        {(['L', 'R'] as const).map((side) => (
          // A hair larger than the eye, so a closing lid also covers its antialiased rim.
          <clipPath key={side} id={`${id}eye${side}`}>
            <ellipse cx={EYES[side]} cy={EYES.y} rx={geo.eye.rx + 1.5} ry={geo.eye.ry + 1.5} />
          </clipPath>
        ))}
      </defs>
      <g transform={`translate(${-box.x} ${-box.y})`}>
        {variant === 'icon' ? (
          <>
            <path d={squircle(-ox / k, -oy / k, 1024 / k, 1024 / k, 230 / k)} fill={`url(#${id}bg)`} />
            <path d={squircle(-ox / k, -oy / k, 1024 / k, 1024 / k, 230 / k)} fill={`url(#${id}sky)`} />
            <path d={squircle((2 - ox) / k, (2 - oy) / k, 1020 / k, 1020 / k, 228 / k)} fill="none" stroke="#fff" strokeOpacity="0.12" strokeWidth={4 / k} />
            {/* Stays on the ground while the body hops. */}
            <ellipse cx="512" cy={geo.feet + (geo.stand ? 2 : 22)} rx={geo.stand ? 210 : 250} ry={geo.stand ? 22 : 30} fill={`url(#${id}shadow)`} />
          </>
        ) : null}
        <g data-part="char">
          <g data-part="body" style={{ transformOrigin: `512px ${geo.feet}px` }}>
            <g data-part="bodyM" style={{ transformOrigin: `512px ${geo.feet}px` }}>
              {geo.stand ? (
                <>
                  {variant === 'glyph' ? (
                    <g className={styles.outline}>
                      <path d={STAND} />
                      <path d={squircle(BASE.x, BASE.y, BASE.w, BASE.h, BASE.h / 2)} />
                    </g>
                  ) : null}
                  <path d={STAND} fill={`url(#${id}metal)`} />
                  <path d={squircle(BASE.x, BASE.y, BASE.w, BASE.h, BASE.h / 2)} fill={`url(#${id}foot)`} />
                </>
              ) : null}
              <g data-part="head" style={{ transformOrigin: HEAD_PIVOT }}>
                <g data-part="headM" style={{ transformOrigin: HEAD_PIVOT }}>
                  {/* The glyph's outline sits behind the ears so it never draws a seam where they join. */}
                  {variant === 'glyph' ? <path className={styles.outline} d={squircle(HEAD.x, HEAD.y, HEAD.w, HEAD.h, HEAD.r)} /> : null}
                  {(['L', 'R'] as const).map((side) => (
                    <g key={side} data-part={`ear${side}`} style={{ transformOrigin: `${EAR_PIVOT[side]}px ${EAR_PIVOT.y}px` }}>
                      <g data-part={`ear${side}M`} style={{ transformOrigin: `${EAR_PIVOT[side]}px ${EAR_PIVOT.y}px` }}>
                        <g className={`${styles.ear} ${styles[`ear${side}`]}`} style={{ transformOrigin: `${EAR_PIVOT[side]}px ${EAR_PIVOT.y}px` }}>
                          <path className={variant === 'glyph' ? styles.edge : undefined} d={EAR_PATH[side]} fill={`url(#${id}face)`} />
                          {geo.stand ? <path d={INNER_EAR_PATH[side]} fill={`url(#${id}innerEar)`} /> : null}
                        </g>
                      </g>
                    </g>
                  ))}
                  <path d={squircle(HEAD.x, HEAD.y, HEAD.w, HEAD.h, HEAD.r)} fill={`url(#${id}face)`} />
                  <path d={squircle(s.x - 4, s.y - 4, s.w + 8, s.h + 8, s.r + 4)} fill={`url(#${id}recess)`} />
                  <path d={squircle(s.x, s.y, s.w, s.h, s.r)} fill={`url(#${id}screen)`} />
                  <g clipPath={`url(#${id}screenClip)`}>
                    <rect x={s.x} y={s.y} width={s.w} height={s.h} fill={`url(#${id}inset)`} />
                    {/* With a mouth, the eyes rise so eyes, nose and mouth sit centred on the screen together. */}
                    <g transform={`translate(0 ${geo.stand ? FACE_RISE : 0})`}>
                      <g data-part="look" style={{ transformOrigin: '512px 514px' }}>
                        <g data-part="lookM" style={{ transformOrigin: '512px 514px' }}>
                          {(['L', 'R'] as const).map((side) => (
                            <g key={side} data-part={`eye${side}`} style={{ transformOrigin: `${EYES[side]}px ${EYES.y}px` }}>
                              <ellipse cx={EYES[side]} cy={EYES.y} rx={geo.eye.rx * 2.4} ry={geo.eye.ry * 1.8} fill={`url(#${id}eyeGlow)`} />
                              {/* Blinks close a little below center: the upper lid travels further. */}
                              <g data-part={`eye${side}M`} clipPath={`url(#${id}eye${side})`} style={{ transformOrigin: `${EYES[side]}px ${EYES.y + geo.eye.ry * 0.2}px` }}>
                                <ellipse cx={EYES[side]} cy={EYES.y} rx={geo.eye.rx} ry={geo.eye.ry} fill={`url(#${id}eye)`} />
                                <g data-part={`lid${side}`} style={{ transformOrigin: `${EYES[side]}px ${EYES.y}px` }}>
                                  <g data-part={`lid${side}M`} style={{ transformOrigin: `${EYES[side]}px ${EYES.y}px` }}>
                                    <rect x={EYES[side] - geo.eye.rx - 16} y={EYES.y - geo.eye.ry - (2 * geo.eye.ry + 24)} width={2 * geo.eye.rx + 32} height={2 * geo.eye.ry + 24} fill={LID} />
                                  </g>
                                </g>
                              </g>
                            </g>
                          ))}
                          <g data-part="happy" style={{ transformOrigin: '512px 514px' }}>
                            {(['L', 'R'] as const).map((side) => (
                              <path key={side} className={styles.arc} strokeWidth={geo.arc} d={happyArc(EYES[side], geo)} />
                            ))}
                          </g>
                          <g data-part="sleep" style={{ transformOrigin: '512px 514px' }}>
                            {(['L', 'R'] as const).map((side) => (
                              <path key={side} className={styles.arc} strokeWidth={geo.arc} d={sleepArc(EYES[side], geo)} />
                            ))}
                          </g>
                          {/* Too fine to read below 44 px, so the compact face is eyes only. */}
                          {geo.stand ? (
                            <g className={styles.mouth} transform={`translate(512 ${584 + MOUTH_DROP}) scale(1.6) translate(-512 -584)`}>
                              <path d={NOSE} />
                              <g data-part="mouthM" style={{ transformOrigin: MOUTH_ORIGIN }}>
                                {MOUTHS.map((m) => (
                                  <g key={m} data-part={`m_${m}`} style={{ transformOrigin: MOUTH_ORIGIN }}>
                                    {MOUTH[m].fill ? <path className={styles.fill} d={MOUTH[m].fill} fillOpacity={m === 'yawn' ? 0.24 : undefined} /> : null}
                                    {MOUTH[m].stroke ? <path className={styles.arc} strokeWidth="6.5" d={MOUTH[m].stroke} /> : null}
                                  </g>
                                ))}
                              </g>
                            </g>
                          ) : null}
                        </g>
                      </g>
                    </g>
                    <g data-part="glow" style={{ transformOrigin: '512px 514px' }}>
                      <g data-part="glowM" style={{ transformOrigin: '512px 514px' }}>
                        <ellipse cx="512" cy="514" rx={s.w * 0.46} ry={s.h * 0.46} fill={`url(#${id}glow)`} />
                      </g>
                    </g>
                    <rect x={s.x} y={s.y} width={s.w} height={s.h} fill={`url(#${id}glass)`} />
                  </g>
                </g>
              </g>
            </g>
          </g>
        </g>
      </g>
    </svg>
  );
}

/** ^ : a content squint. */
function happyArc(cx: number, geo: Geometry) {
  const w = geo.eye.rx * 0.88;
  const h = geo.eye.ry * 0.21;
  return `M${cx - w} ${EYES.y + h} Q${cx} ${EYES.y - h * 3.4} ${cx + w} ${EYES.y + h}`;
}
/** ‿ : eyes softly shut. */
function sleepArc(cx: number, geo: Geometry) {
  const w = geo.eye.rx * 0.88;
  const h = geo.eye.ry * 0.08;
  return `M${cx - w} ${EYES.y - h} Q${cx} ${EYES.y + h * 9} ${cx + w} ${EYES.y - h}`;
}

/* ───────────────────────── Motion ───────────────────────── */

// Curves (easings.net), named for what they do; the press and ear springs are
// SwiftUI-style springs sampled into linear() so they run off the main thread.
const EASE = {
  inOut: 'cubic-bezier(0.65, 0, 0.35, 1)', // easeInOutCubic: deliberate moves
  sine: 'cubic-bezier(0.37, 0, 0.63, 1)', // easeInOutSine: breathing, drift
  out: 'cubic-bezier(0.33, 1, 0.68, 1)', // easeOutCubic: saccades, arrivals
  in: 'cubic-bezier(0.32, 0, 0.67, 0)', // easeInCubic: falling, lids closing
  inQuad: 'cubic-bezier(0.11, 0, 0.5, 0)', // first half of an arc
  outQuad: 'cubic-bezier(0.5, 1, 0.89, 1)', // second half of an arc
  snap: 'cubic-bezier(0.23, 1, 0.32, 1)', // the app's --ease-out
  // bounce 0.35: ears and eyes snapping to attention.
  spring:
    'linear(0, 0.028, 0.099, 0.197, 0.311, 0.429, 0.545, 0.652, 0.748, 0.831, 0.9, 0.955, 0.997, 1.028, 1.049, 1.061, 1.067, 1.068, 1.065, 1.059, 1.052, 1.044, 1.036, 1.028, 1.021, 1.015, 1.01, 1.005, 1.002, 0.999, 0.998, 0.996, 0.996, 0.995, 0.995, 0.996, 1)',
  // bounce 0.25: ears settling after the head, the press release.
  springSoft:
    'linear(0, 0.021, 0.074, 0.15, 0.238, 0.332, 0.426, 0.517, 0.602, 0.679, 0.747, 0.807, 0.858, 0.9, 0.935, 0.962, 0.984, 1, 1.012, 1.02, 1.025, 1.028, 1.028, 1.028, 1.026, 1.024, 1.022, 1.019, 1.016, 1.014, 1.011, 1.009, 1.007, 1.006, 1.004, 1.003, 1)',
};

/** A resting expression. Ears: + perks (toward upright), − droops. Lid: 0 open to 1 shut. */
type Pose = {
  head: [y: number, deg: number];
  ears: [y: number, perk: number];
  /** Extra perk on the right ear, for an asymmetric, curious look. */
  earR?: [y: number, perk: number];
  look: [x: number, y: number];
  eye: [sx: number, sy: number];
  closed: 'happy' | 'sleep' | null;
  lid: [amount: number, sad: number];
  glow: [scale: number, opacity: number];
  dim: number;
  mouth: MouthName;
};

const POSES: Record<FamiliarState, Pose> = {
  idle: { head: [0, 0], ears: [0, 0], look: [0, 0], eye: [1, 1], closed: null, lid: [0, 0], glow: [1, 0], dim: 1, mouth: 'cat' },
  thinking: { head: [0, -2.5], ears: [-6, 5], earR: [0, -6], look: [8, -12], eye: [1, 1], closed: null, lid: [0.12, 0], glow: [1, 0], dim: 1, mouth: 'hmm' },
  happy: { head: [-2, 0], ears: [-10, 9], look: [0, -4], eye: [1, 0.1], closed: 'happy', lid: [0, 0], glow: [1.04, 0.5], dim: 1, mouth: 'grin' },
  attention: { head: [-4, 0], ears: [-16, 12], look: [0, -6], eye: [1.18, 1.18], closed: null, lid: [0, 0], glow: [1, 0], dim: 1, mouth: 'o' },
  sleeping: { head: [6, 3], ears: [6, -14], look: [0, 12], eye: [1, 0.1], closed: 'sleep', lid: [0, 0], glow: [1, 0.7], dim: 1, mouth: 'snooze' },
  waking: { head: [0, 0], ears: [0, 0], look: [0, 0], eye: [1, 1], closed: null, lid: [0, 0], glow: [1, 0], dim: 1, mouth: 'cat' },
  sad: { head: [8, 0], ears: [8, -17], look: [0, 18], eye: [0.94, 0.9], closed: null, lid: [0.16, 24], glow: [1, 0], dim: 1, mouth: 'frown' },
  offline: { head: [4, 0], ears: [6, -10], look: [0, 8], eye: [1, 1], closed: null, lid: [0.5, 0], glow: [1, 0], dim: 0.55, mouth: 'flat' },
};

type PartName =
  | 'char' | 'body' | 'bodyM' | 'head' | 'headM' | 'earL' | 'earLM' | 'earR' | 'earRM' | 'look' | 'lookM'
  | 'eyeL' | 'eyeLM' | 'eyeR' | 'eyeRM' | 'lidL' | 'lidLM' | 'lidR' | 'lidRM' | 'happy' | 'sleep' | 'glow' | 'glowM'
  | 'mouthM' | `m_${MouthName}`;

type Frame = Keyframe & { transform?: string; opacity?: number };

// Each part keeps one transform function list, so every interpolation is per-function and exact.
const T = {
  move: (y: number, deg: number) => `translate(0px, ${r(y)}px) rotate(${r(deg)}deg)`,
  look: (x: number, y: number) => `translate(${r(x)}px, ${r(y)}px)`,
  scale: (sx: number, sy: number) => `scale(${r(sx)}, ${r(sy)})`,
  body: (y: number, sx: number, sy: number) => `translate(0px, ${r(y)}px) scale(${r(sx)}, ${r(sy)})`,
};
function r(value: number) {
  return Math.round(value * 1000) / 1000;
}
function between(min: number, max: number) {
  return min + Math.random() * (max - min);
}

/** Values for each pose part. Ear perk is mirrored: the left ear perks clockwise, the right counter-clockwise. */
function values(pose: Pose, geo: Geometry): Partial<Record<PartName, Frame>> {
  const right = pose.earR ?? pose.ears;
  const lid = pose.lid[0] * 2 * geo.eye.ry;
  return {
    char: { opacity: pose.dim },
    head: { transform: T.move(...pose.head) },
    earL: { transform: T.move(pose.ears[0], pose.ears[1]) },
    earR: { transform: T.move(right[0], -right[1]) },
    look: { transform: T.look(...pose.look) },
    eyeL: { transform: T.scale(...pose.eye), opacity: pose.closed ? 0 : 1 },
    eyeR: { transform: T.scale(...pose.eye), opacity: pose.closed ? 0 : 1 },
    lidL: { transform: T.move(lid, -pose.lid[1]) },
    lidR: { transform: T.move(lid, pose.lid[1]) },
    happy: { transform: T.scale(1, pose.closed === 'happy' ? 1 : 0.3), opacity: pose.closed === 'happy' ? 1 : 0 },
    sleep: { transform: T.scale(1, pose.closed === 'sleep' ? 1 : 0.3), opacity: pose.closed === 'sleep' ? 1 : 0 },
    glow: { transform: T.scale(pose.glow[0], pose.glow[0]), opacity: pose.glow[1] },
    ...Object.fromEntries(MOUTHS.map((m) => [`m_${m}`, pose.mouth === m ? { transform: T.scale(1, 1), opacity: 1 } : { transform: T.scale(0.5, 0.5), opacity: 0 }])),
  };
}

const REST: Partial<Record<PartName, Frame>> = {
  bodyM: { transform: T.body(0, 1, 1) },
  headM: { transform: T.move(0, 0) },
  earLM: { transform: T.move(0, 0) },
  earRM: { transform: T.move(0, 0) },
  lookM: { transform: T.look(0, 0) },
  eyeLM: { transform: T.scale(1, 1) },
  eyeRM: { transform: T.scale(1, 1) },
  lidLM: { transform: T.move(0, 0) },
  lidRM: { transform: T.move(0, 0) },
  glowM: { transform: T.scale(1, 1), opacity: 1 },
  mouthM: { transform: T.look(0, 0) },
};

type Timing = { duration: number; easing: string; delay?: number };

/**
 * Plays Familiar. Every motion is a Web Animation on transform or opacity, so it
 * is time-based and runs at the display's refresh rate. Random behaviour (blinks,
 * twitches, glances) is scheduled with animation delays rather than timers, so
 * pausing the animations pauses the schedule too.
 */
class Director {
  private parts = new Map<PartName, SVGGElement>();
  private state: FamiliarState | null = null;
  private pose: Pose | null = null;
  private geo: Geometry = REGULAR;
  private reduced = false;
  /** Bumped on every state change; chains from an older state stop rescheduling. */
  private generation = 0;
  private visible = true;
  private onScreen = true;
  private held = new Set<Animation>();
  private pressAnim: Animation | null = null;
  private observer: IntersectionObserver | null = null;

  constructor(private svg: SVGSVGElement) {
    for (const el of svg.querySelectorAll<SVGGElement>('[data-part]')) this.parts.set(el.dataset.part as PartName, el);
    this.visible = document.visibilityState !== 'hidden';
    document.addEventListener('visibilitychange', this.onVisibility);
    if (typeof IntersectionObserver !== 'undefined') {
      this.observer = new IntersectionObserver((entries) => {
        this.onScreen = entries.some((entry) => entry.isIntersecting);
        this.sync();
      });
      this.observer.observe(svg);
    }
  }

  destroy() {
    this.generation++;
    document.removeEventListener('visibilitychange', this.onVisibility);
    this.observer?.disconnect();
    for (const animation of this.svg.getAnimations({ subtree: true })) animation.cancel();
  }

  set(state: FamiliarState, geo: Geometry, reduced: boolean) {
    const first = this.state === null || geo !== this.geo;
    if (!first && state === this.state && reduced === this.reduced) return;
    const from = this.pose;
    this.geo = geo;
    this.reduced = reduced;
    this.state = state;
    this.pose = POSES[state];
    const generation = ++this.generation;
    this.freeze();
    // Waking on arrival starts from asleep.
    if (first && state === 'waking' && !reduced) this.apply(values(POSES.sleeping, geo));

    if (reduced) {
      this.crossfade(first);
    } else if (first && state !== 'happy' && state !== 'attention' && state !== 'waking') {
      this.apply(values(this.pose, geo));
      this.apply(REST);
      this.loop(state, generation);
    } else {
      const settled = this.enter(state, from, first);
      void Promise.all(settled.map((animation) => animation.finished)).then(
        () => generation === this.generation && this.loop(state, generation),
        () => {},
      );
    }
    this.sync();
  }

  /* Interaction: a squish on press with a springy release, on the root element so it is composited. */
  press() {
    const current = getComputedStyle(this.svg).transform;
    for (const animation of this.svg.getAnimations()) animation.cancel();
    this.svg.style.transform = current === 'none' ? '' : current;
    this.pressAnim = this.svg.animate([{ transform: 'scale(1.03, 0.94)' }], { duration: 110, easing: EASE.out, fill: 'forwards' });
  }

  release() {
    const down = this.pressAnim;
    if (!down) return;
    this.pressAnim = null;
    // Let a quick tap reach the full squish before it springs back.
    void down.finished.then(
      () => {
        if (this.pressAnim) return;
        const back = this.svg.animate([{ transform: 'scale(1.03, 0.94)' }, { transform: 'scale(1, 1)' }], { duration: 560, easing: EASE.springSoft });
        down.cancel();
        this.svg.style.transform = '';
        this.track(back);
      },
      () => {},
    );
  }

  /* ── Plumbing ── */

  private onVisibility = () => {
    this.visible = document.visibilityState !== 'hidden';
    this.sync();
  };

  /** Pauses everything while hidden or scrolled away; resumes where it left off. */
  private sync() {
    const active = this.visible && this.onScreen;
    if (!active) {
      for (const animation of this.svg.getAnimations({ subtree: true })) {
        if (animation.playState === 'running') {
          animation.pause();
          this.held.add(animation);
        }
      }
    } else if (this.held.size) {
      for (const animation of this.held) if (animation.playState === 'paused') animation.play();
      this.held.clear();
    }
  }

  private track(animation: Animation) {
    if (!(this.visible && this.onScreen)) {
      animation.pause();
      this.held.add(animation);
    }
    return animation;
  }

  /** Holds every part exactly where it is mid-motion, so the next state starts from there. */
  private freeze() {
    const moving: [SVGGElement, string, string][] = [];
    for (const el of this.parts.values()) {
      if (!el.getAnimations().length) continue;
      const style = getComputedStyle(el);
      moving.push([el, style.transform, style.opacity]);
    }
    for (const [el, transform, opacity] of moving) {
      el.style.transform = transform === 'none' ? '' : transform;
      el.style.opacity = opacity;
      for (const animation of el.getAnimations()) animation.cancel();
    }
    this.held.clear();
  }

  /** Where a part is right now (after a freeze), to start a one-shot from it. */
  private now(name: PartName) {
    return this.parts.get(name)?.style.transform || REST[name]?.transform || 'none';
  }

  private apply(frames: Partial<Record<PartName, Frame>>) {
    for (const [name, frame] of Object.entries(frames) as [PartName, Frame][]) {
      const el = this.parts.get(name);
      if (!el) continue;
      if (frame.transform !== undefined) el.style.transform = frame.transform;
      if (frame.opacity !== undefined) el.style.opacity = String(frame.opacity);
    }
  }

  /** Animates a part; a finished one-shot writes its last frame inline and lets go. */
  private play(name: PartName, frames: Frame[], timing: KeyframeAnimationOptions) {
    const el = this.parts.get(name);
    if (!el) return null;
    const animation = el.animate(frames, { fill: 'forwards', ...timing });
    if (timing.iterations !== Infinity) {
      const last = frames[frames.length - 1]!;
      animation.onfinish = () => {
        if (last.transform !== undefined) el.style.transform = last.transform;
        if (last.opacity !== undefined) el.style.opacity = String(last.opacity);
        animation.cancel();
      };
    }
    return this.track(animation);
  }

  /** Reduced motion: no movement, just a cross-fade from the old expression to the new one. */
  private crossfade(first: boolean) {
    const char = this.parts.get('char')!;
    const target = values(this.pose!, this.geo);
    if (!first) {
      const ghost = char.cloneNode(true) as SVGGElement;
      ghost.removeAttribute('data-part');
      for (const el of ghost.querySelectorAll('[data-part]')) el.removeAttribute('data-part');
      char.after(ghost);
      const fade = ghost.animate([{ opacity: 0 }], { duration: 240, easing: EASE.sine, fill: 'forwards' });
      fade.onfinish = () => ghost.remove();
      fade.oncancel = () => ghost.remove();
      this.track(fade);
    }
    this.apply(target);
    this.apply(REST);
    if (!first) this.play('char', [{ opacity: 0 }, { opacity: this.pose!.dim }], { duration: 240, easing: EASE.sine });
  }

  /* ── Choreography ── */

  /** Moves into a state from wherever Familiar is. Returns what must finish before the state's loops start. */
  private enter(state: FamiliarState, from: Pose | null, first: boolean): Animation[] {
    const to = values(this.pose!, this.geo);
    const out: (Animation | null)[] = [];
    const go = (name: PartName, timing: Timing, frames: Frame[] = [to[name]!]) => out.push(this.play(name, frames, timing));

    // Motion layers drift home from wherever a loop left them.
    if (!first) for (const [name, frame] of Object.entries(REST) as [PartName, Frame][]) this.play(name, [frame], { duration: 280, easing: EASE.out });

    if (state === 'waking') return this.wake(out);

    const t = TIMINGS[state];
    go('char', t.base);
    go('head', t.head ?? t.base);
    go('look', t.look ?? t.base);
    go('glow', t.base);
    go('earL', t.ears);
    go('earR', { ...t.ears, delay: (t.ears.delay ?? 0) + 40 });
    if (state === 'sleeping' && !from?.closed) {
      // Falling asleep: the lids droop first, then the eyes close behind them.
      const droop = 0.55 * 2 * this.geo.eye.ry;
      go('lidL', t.base, [{ transform: T.move(droop, 0), offset: 0.55, easing: EASE.sine }, to.lidL!]);
      go('lidR', t.base, [{ transform: T.move(droop, 0), offset: 0.55, easing: EASE.sine }, to.lidR!]);
    } else {
      go('lidL', t.lids ?? t.base);
      go('lidR', t.lids ?? t.base);
    }
    this.eyes(go, to, from, t.eyes ?? t.base);
    // Mouths share an origin, so overlapping them reads as one mouth changing shape:
    // the old one holds a beat and shrinks away as the new one grows in over it.
    const mouth = t.mouth ?? { duration: 360, easing: EASE.inOut };
    const away = { duration: Math.max(200, mouth.duration * 0.55), delay: mouth.delay, easing: EASE.in };
    for (const m of MOUTHS) {
      const showing = !!to[`m_${m}`]!.opacity;
      if (showing && from?.mouth === m) continue;
      go(`m_${m}`, showing ? mouth : away);
    }

    if (state === 'happy') out.push(...this.hop());
    if (state === 'attention') out.push(this.play('bodyM', [{ transform: this.now('bodyM'), offset: 0, easing: EASE.inOut }, { transform: T.body(2, 1.02, 0.97), offset: 0.2, easing: EASE.out }, { transform: T.body(-8, 0.99, 1.025), offset: 0.5, easing: EASE.inOut }, { transform: T.body(0, 1, 1) }], { duration: 520, easing: 'linear' }));
    if (state === 'sad') out.push(this.play('bodyM', [{ transform: this.now('bodyM'), offset: 0, easing: EASE.inOut }, { transform: T.body(4, 1.02, 0.975), offset: 0.55, easing: EASE.inOut }, { transform: T.body(0, 1, 1) }], { duration: 1100, easing: 'linear', delay: 120 }));
    return out.filter((animation): animation is Animation => !!animation);
  }

  /** Eyes open or close through the arcs: the ellipse flattens into a line as the curve grows out of it. */
  private eyes(go: (name: PartName, timing: Timing, frames?: Frame[]) => void, to: Partial<Record<PartName, Frame>>, from: Pose | null, timing: Timing) {
    const closing = !!this.pose!.closed && !from?.closed;
    const opening = !this.pose!.closed && !!from?.closed;
    for (const eye of ['eyeL', 'eyeR'] as const) {
      const target = to[eye]!;
      if (closing) go(eye, timing, [{ transform: T.scale(1, 0.2), opacity: 1, offset: 0.6 }, target]);
      else if (opening) go(eye, timing, [{ transform: T.scale(1, 0.18), opacity: 1, offset: 0.35 }, target]);
      else go(eye, timing, [target]);
    }
    for (const arc of ['happy', 'sleep'] as const) {
      const target = to[arc]!;
      const showing = target.opacity === 1;
      go(arc, timing, showing ? [{ transform: T.scale(1, 0.3), opacity: 0, offset: closing ? 0.5 : 0.3 }, target] : [{ transform: T.scale(1, 0.3), opacity: 0, offset: 0.4 }, target]);
    }
  }

  /** happy: anticipation squash, stretch on launch, squash on landing; the head and ears follow through. */
  private hop(): (Animation | null)[] {
    const kick = { duration: 780, easing: 'linear' };
    const ear = (sign: 1 | -1, name: PartName) => [
      { transform: this.now(name), offset: 0, easing: EASE.out },
      { transform: T.move(0, 4 * sign), offset: 0.16, easing: EASE.inOut },
      { transform: T.move(0, -9 * sign), offset: 0.42, easing: EASE.inOut },
      { transform: T.move(0, 6 * sign), offset: 0.58, easing: EASE.inOut },
      { transform: T.move(0, -10 * sign), offset: 0.8, easing: EASE.out },
      { transform: T.move(0, 3 * sign), offset: 0.9, easing: EASE.sine },
      { transform: T.move(0, 0) },
    ];
    return [
      this.play(
        'bodyM',
        [
          { transform: this.now('bodyM'), offset: 0, easing: EASE.sine },
          { transform: T.body(0, 1.05, 0.92), offset: 0.18, easing: EASE.out },
          { transform: T.body(-44, 0.97, 1.05), offset: 0.4, easing: EASE.out },
          { transform: T.body(-52, 1, 1), offset: 0.52, easing: EASE.in },
          { transform: T.body(0, 0.985, 1.025), offset: 0.72, easing: EASE.out },
          { transform: T.body(0, 1.04, 0.95), offset: 0.8, easing: EASE.inOut },
          { transform: T.body(0, 1, 1) },
        ],
        kick,
      ),
      this.play(
        'headM',
        [
          { transform: this.now('headM'), offset: 0, easing: EASE.sine },
          { transform: T.move(4, 0), offset: 0.2, easing: EASE.out },
          { transform: T.move(-5, 0), offset: 0.46, easing: EASE.inOut },
          { transform: T.move(0, 0), offset: 0.76, easing: EASE.out },
          { transform: T.move(5, 0), offset: 0.86, easing: EASE.inOut },
          { transform: T.move(0, 0) },
        ],
        kick,
      ),
      this.play('earLM', ear(1, 'earLM'), { duration: 920, easing: 'linear', delay: 30 }),
      this.play('earRM', ear(-1, 'earRM'), { duration: 920, easing: 'linear', delay: 70 }),
    ];
  }

  /** waking: a stretch with the eyes still shut, a sleepy half-open, blink-blink, then bright. */
  private wake(out: (Animation | null)[]): Animation[] {
    const geo = this.geo;
    const lid = (amount: number) => amount * 2 * geo.eye.ry;
    const run = { duration: 1900, easing: 'linear' };
    out.push(
      this.play(
        'bodyM',
        [
          { transform: T.body(2, 1.02, 0.97), offset: 0.12, easing: EASE.inOut },
          { transform: T.body(-8, 0.965, 1.06), offset: 0.36, easing: EASE.sine },
          { transform: T.body(-9, 0.97, 1.055), offset: 0.46, easing: EASE.in },
          { transform: T.body(1, 1.025, 0.975), offset: 0.56, easing: EASE.out },
          { transform: T.body(0, 1, 1), offset: 0.68 },
          { transform: T.body(0, 1, 1) },
        ],
        run,
      ),
      this.play('head', [{ transform: T.move(-4, -3), offset: 0.38, easing: EASE.inOut }, { transform: T.move(0, 1.2), offset: 0.6, easing: EASE.sine }, { transform: T.move(0, 0), offset: 0.74 }, { transform: T.move(0, 0) }], run),
      this.play('earL', [{ transform: T.move(-10, 12), offset: 0.38, easing: EASE.sine }, { transform: T.move(-10, 11), offset: 0.48, easing: EASE.inOut }, { transform: T.move(0, -3), offset: 0.62, easing: EASE.sine }, { transform: T.move(0, 0.8), offset: 0.74, easing: EASE.sine }, { transform: T.move(0, 0) }], run),
      this.play('earR', [{ transform: T.move(-10, -12), offset: 0.4, easing: EASE.sine }, { transform: T.move(-10, -11), offset: 0.5, easing: EASE.inOut }, { transform: T.move(0, 3), offset: 0.64, easing: EASE.sine }, { transform: T.move(0, -0.8), offset: 0.76, easing: EASE.sine }, { transform: T.move(0, 0) }], run),
      this.play('look', [{ transform: T.look(0, 10), offset: 0.5, easing: EASE.inOut }, { transform: T.look(0, 0), offset: 0.92 }, { transform: T.look(0, 0) }], run),
      this.play('glow', [{ transform: T.scale(1, 1), opacity: 0, offset: 0.6 }, { transform: T.scale(1, 1), opacity: 0 }], run),
      this.play('char', [{ opacity: 1 }], { duration: 400, easing: EASE.sine }),
      this.play('sleep', [{ transform: T.scale(1, 1), opacity: 1, offset: 0.46, easing: EASE.in }, { transform: T.scale(1, 0.3), opacity: 0, offset: 0.52 }, { transform: T.scale(1, 0.3), opacity: 0 }], run),
      this.play('happy', [{ transform: T.scale(1, 0.3), opacity: 0 }], { duration: 300, easing: EASE.out }),
      // A yawn at the top of the stretch: the sleepy mouth opens wide, holds, and closes into the resting one.
      this.play('m_snooze', [{ transform: T.scale(1, 1), opacity: 1, offset: 0.14, easing: EASE.out }, { transform: T.scale(0.5, 0.5), opacity: 0, offset: 0.2 }, { transform: T.scale(0.5, 0.5), opacity: 0 }], run),
      this.play(
        'm_yawn',
        [
          { transform: T.scale(0.4, 0.2), opacity: 0, offset: 0.15, easing: EASE.out },
          { transform: T.scale(0.85, 0.8), opacity: 1, offset: 0.24, easing: EASE.sine },
          { transform: T.scale(1, 1), opacity: 1, offset: 0.4, easing: EASE.sine },
          { transform: T.scale(1.04, 1.03), opacity: 1, offset: 0.46, easing: EASE.in },
          { transform: T.scale(0.6, 0.15), opacity: 0, offset: 0.54 },
          { transform: T.scale(0.4, 0.2), opacity: 0 },
        ],
        run,
      ),
      this.play('m_cat', [{ transform: T.scale(0.5, 0.5), opacity: 0, offset: 0.54, easing: EASE.out }, { transform: T.scale(1, 1), opacity: 1, offset: 0.62 }, { transform: T.scale(1, 1), opacity: 1 }], run),
    );
    for (const side of ['L', 'R'] as const) {
      // The eyes come back half-lidded, blink twice, and the lids lift as it comes to.
      out.push(
        this.play(
          `eye${side}`,
          [
            { transform: T.scale(1, 0.1), opacity: 0, offset: 0.5, easing: EASE.out },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.58 },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.63, easing: EASE.in },
            { transform: T.scale(1, 0.08), opacity: 1, offset: 0.66, easing: EASE.out },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.7 },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.72, easing: EASE.in },
            { transform: T.scale(1, 0.08), opacity: 1, offset: 0.745, easing: EASE.out },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.79 },
            { transform: T.scale(1, 1), opacity: 1 },
          ],
          run,
        ),
        this.play(
          `lid${side}`,
          [
            { transform: T.move(lid(0.5), 0), offset: 0.5, easing: EASE.inOut },
            { transform: T.move(lid(0.45), 0), offset: 0.7, easing: EASE.inOut },
            { transform: T.move(0, 0), offset: 0.92 },
            { transform: T.move(0, 0) },
          ],
          run,
        ),
      );
    }
    return out.filter((animation): animation is Animation => !!animation);
  }

  /* ── Life: what each state does while it rests ── */

  private loop(state: FamiliarState, generation: number) {
    switch (state) {
      case 'idle':
      case 'waking':
        this.blinks(generation, [2200, 6400], 190);
        this.twitches(generation, [6000, 14000]);
        this.glances(generation);
        this.bleps(generation);
        break;
      case 'thinking':
        this.scan();
        this.blinks(generation, [2400, 5200], 180);
        this.twitches(generation, [9000, 16000]);
        break;
      case 'happy':
        this.twitches(generation, [5000, 11000]);
        break;
      case 'attention':
        this.blinks(generation, [3000, 6000], 170);
        this.flicks(generation);
        break;
      case 'sleeping':
        this.breathe();
        break;
      case 'sad':
        this.blinks(generation, [3800, 7000], 340);
        this.sighs(generation);
        break;
      case 'offline':
        this.drowse(generation);
        break;
    }
  }

  /**
   * Repeats a behaviour after a random pause, for as long as the state lasts.
   * The first beat comes sooner, so a Familiar that just appeared shows some life.
   */
  private every(generation: number, range: [number, number], run: (delay: number) => (Animation | null)[]) {
    let first = true;
    const next = () => {
      if (generation !== this.generation) return;
      const delay = first ? between(range[0] * 0.3, range[0]) : between(...range);
      first = false;
      const animations = run(delay).filter((animation): animation is Animation => !!animation);
      if (!animations.length) return;
      void Promise.all(animations.map((animation) => animation.finished)).then(next, () => {});
    };
    next();
  }

  /** Irregular blinks: mostly single, sometimes a double, never on a beat. */
  private blinks(generation: number, range: [number, number], duration: number) {
    this.every(generation, range, (delay) => {
      const double = Math.random() < 0.18;
      const shut = T.scale(1, 0.06);
      const open = T.scale(1, 1);
      const frames: Frame[] = double
        ? [
            { transform: open, easing: EASE.in },
            { transform: shut, offset: 0.17, easing: 'linear' },
            { transform: shut, offset: 0.21, easing: EASE.out },
            { transform: open, offset: 0.44 },
            { transform: open, offset: 0.54, easing: EASE.in },
            { transform: shut, offset: 0.7, easing: 'linear' },
            { transform: shut, offset: 0.74, easing: EASE.out },
            { transform: open },
          ]
        : [{ transform: open, easing: EASE.in }, { transform: shut, offset: 0.38, easing: 'linear' }, { transform: shut, offset: 0.48, easing: EASE.out }, { transform: open }];
      const timing = { duration: double ? duration * 2.3 : duration, delay, easing: 'linear' };
      return [this.play('eyeLM', frames, timing), this.play('eyeRM', frames, timing)];
    });
  }

  /** One ear flicks out and settles with a small overshoot; sometimes the other answers. */
  private twitches(generation: number, range: [number, number]) {
    this.every(generation, range, (delay) => {
      const left = Math.random() < 0.5;
      const flick = (sign: number, size: number): Frame[] => [
        { transform: T.move(0, 0), easing: EASE.out },
        { transform: T.move(0, -10 * size * sign), offset: 0.14, easing: EASE.sine },
        { transform: T.move(0, 3 * size * sign), offset: 0.42, easing: EASE.sine },
        { transform: T.move(0, -1 * size * sign), offset: 0.68, easing: EASE.sine },
        { transform: T.move(0, 0) },
      ];
      const timing = { duration: 560, delay, easing: 'linear' };
      const out = [this.play(left ? 'earLM' : 'earRM', flick(left ? 1 : -1, 1), timing)];
      if (Math.random() < 0.4) out.push(this.play(left ? 'earRM' : 'earLM', flick(left ? -1 : 1, 0.3), { ...timing, delay: delay + 70 }));
      return out;
    });
  }

  /** Rare, slow glances along a slight arc, with the head leaning after the eyes. */
  private glances(generation: number) {
    this.every(generation, [9000, 18000], (delay) => {
      const side = Math.random() < 0.5 ? -1 : 1;
      const x = side * between(14, 24);
      const y = between(-9, 6);
      const duration = between(2400, 3400);
      const hold = between(0.62, 0.74);
      const timing = { duration, delay, easing: 'linear' };
      const out = [
        this.play(
          'lookM',
          [
            { transform: T.look(0, 0), easing: EASE.inQuad },
            { transform: T.look(x * 0.5, y * 0.5 - 5), offset: 0.07, easing: EASE.outQuad },
            { transform: T.look(x, y), offset: 0.14 },
            { transform: T.look(x, y), offset: hold, easing: EASE.inQuad },
            { transform: T.look(x * 0.5, y * 0.5 - 4), offset: hold + 0.08, easing: EASE.outQuad },
            { transform: T.look(0, 0), offset: hold + 0.16 },
            { transform: T.look(0, 0) },
          ],
          timing,
        ),
        this.play(
          'headM',
          [
            { transform: T.move(0, 0), offset: 0, easing: EASE.inOut },
            { transform: T.move(0, side * 1.4), offset: 0.22, easing: EASE.sine },
            { transform: T.move(0, side * 1.4), offset: hold + 0.02, easing: EASE.inOut },
            { transform: T.move(0, 0), offset: hold + 0.22 },
            { transform: T.move(0, 0) },
          ],
          timing,
        ),
      ];
      // Eyes often blink as they jump, as real ones do.
      if (Math.random() < 0.4) {
        const blink: Frame[] = [{ transform: T.scale(1, 1), easing: EASE.in }, { transform: T.scale(1, 0.06), offset: 0.38, easing: 'linear' }, { transform: T.scale(1, 0.06), offset: 0.48, easing: EASE.out }, { transform: T.scale(1, 1) }];
        for (const eye of ['eyeLM', 'eyeRM'] as const) this.play(eye, blink, { duration: 190, delay: delay - 30, easing: 'linear' });
      }
      return out;
    });
  }

  /** idle, rarely: the tongue tip slips out, stays a moment too long, and pops back in. */
  private bleps(generation: number) {
    this.every(generation, [16000, 32000], (delay) => {
      const timing = { duration: 1500, delay, easing: 'linear' };
      return [
        this.play(
          'm_blep',
          [
            { transform: T.scale(0.5, 0.2), opacity: 0, easing: EASE.out },
            { transform: T.scale(1, 1.08), opacity: 1, offset: 0.12, easing: EASE.sine },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.2 },
            { transform: T.scale(1, 1), opacity: 1, offset: 0.82, easing: EASE.in },
            { transform: T.scale(0.6, 0.2), opacity: 0, offset: 0.9 },
            { transform: T.scale(0.5, 0.2), opacity: 0 },
          ],
          timing,
        ),
      ];
    });
  }

  /** thinking: the eyes scan in saccades with holds, and the head sways a beat behind. */
  private scan() {
    const loop = { duration: 4200, iterations: Infinity, easing: 'linear' };
    this.play(
      'lookM',
      [
        { transform: T.look(0, 0), easing: EASE.out },
        { transform: T.look(-34, 6), offset: 0.06 },
        { transform: T.look(-34, 6), offset: 0.3, easing: EASE.inQuad },
        { transform: T.look(-8, -6), offset: 0.36, easing: EASE.outQuad },
        { transform: T.look(26, 4), offset: 0.42 },
        { transform: T.look(26, 4), offset: 0.66, easing: EASE.inOut },
        { transform: T.look(0, 0), offset: 0.8 },
        { transform: T.look(0, 0) },
      ],
      loop,
    );
    this.play(
      'headM',
      [
        { transform: T.move(0, 0), easing: EASE.sine },
        { transform: T.move(0, -1.6), offset: 0.14, easing: EASE.sine },
        { transform: T.move(0, -1.6), offset: 0.33, easing: EASE.sine },
        { transform: T.move(0, 1.6), offset: 0.5, easing: EASE.sine },
        { transform: T.move(0, 1.6), offset: 0.7, easing: EASE.sine },
        { transform: T.move(0, 0), offset: 0.88 },
        { transform: T.move(0, 0) },
      ],
      loop,
    );
    // The mouth slides a little toward each side the eyes check, pursed in thought.
    this.play(
      'mouthM',
      [
        { transform: T.look(0, 0), easing: EASE.out },
        { transform: T.look(-7, 1), offset: 0.08 },
        { transform: T.look(-7, 1), offset: 0.32, easing: EASE.inOut },
        { transform: T.look(5, 0), offset: 0.44 },
        { transform: T.look(5, 0), offset: 0.66, easing: EASE.inOut },
        { transform: T.look(0, 0), offset: 0.8 },
        { transform: T.look(0, 0) },
      ],
      loop,
    );
  }

  /** sleeping: slow breaths; the glow dims on the exhale and the ears ride it a beat late. */
  private breathe() {
    const loop = { duration: 5200, iterations: Infinity, easing: 'linear' };
    this.play('glowM', [{ transform: T.scale(1, 1), opacity: 1, easing: EASE.sine }, { transform: T.scale(0.94, 0.94), opacity: 0.35, offset: 0.55, easing: EASE.sine }, { transform: T.scale(1, 1), opacity: 1 }], loop);
    this.play('bodyM', [{ transform: T.body(0, 1, 1), easing: EASE.sine }, { transform: T.body(1.5, 1.004, 0.988), offset: 0.55, easing: EASE.sine }, { transform: T.body(0, 1, 1) }], loop);
    this.play('earLM', [{ transform: T.move(0, 0), easing: EASE.sine }, { transform: T.move(1, -2), offset: 0.55, easing: EASE.sine }, { transform: T.move(0, 0) }], { ...loop, delay: 260 });
    this.play('earRM', [{ transform: T.move(0, 0), easing: EASE.sine }, { transform: T.move(1, 2), offset: 0.55, easing: EASE.sine }, { transform: T.move(0, 0) }], { ...loop, delay: 320 });
  }

  /** attention: both ears flick a touch higher every few seconds, staying alert. */
  private flicks(generation: number) {
    this.every(generation, [4000, 8000], (delay) => {
      const flick = (sign: number): Frame[] => [
        { transform: T.move(0, 0), easing: EASE.out },
        { transform: T.move(-3, 5 * sign), offset: 0.18, easing: EASE.sine },
        { transform: T.move(0, -1.5 * sign), offset: 0.5, easing: EASE.sine },
        { transform: T.move(0, 0) },
      ];
      return [this.play('earLM', flick(1), { duration: 600, delay, easing: 'linear' }), this.play('earRM', flick(-1), { duration: 600, delay: delay + 40, easing: 'linear' })];
    });
  }

  /** sad: now and then a sigh, and the ears sink with the exhale. */
  private sighs(generation: number) {
    this.every(generation, [7000, 12000], (delay) => {
      const timing = { duration: 1800, delay, easing: 'linear' };
      const ear = (sign: number): Frame[] => [
        { transform: T.move(0, 0), easing: EASE.sine },
        { transform: T.move(-2, 2 * sign), offset: 0.35, easing: EASE.sine },
        { transform: T.move(2, -4 * sign), offset: 0.78, easing: EASE.sine },
        { transform: T.move(0, 0) },
      ];
      return [
        this.play('bodyM', [{ transform: T.body(0, 1, 1), easing: EASE.sine }, { transform: T.body(-3, 0.996, 1.014), offset: 0.35, easing: EASE.sine }, { transform: T.body(2, 1.006, 0.99), offset: 0.75, easing: EASE.sine }, { transform: T.body(0, 1, 1) }], timing),
        this.play('earLM', ear(1), { ...timing, delay: delay + 150 }),
        this.play('earRM', ear(-1), { ...timing, delay: delay + 190 }),
      ];
    });
  }

  /** offline: the lids sink, the head nods, then it catches itself. */
  private drowse(generation: number) {
    this.every(generation, [4500, 9000], (delay) => {
      const timing = { duration: 2400, delay, easing: 'linear' };
      const sink = 0.34 * 2 * this.geo.eye.ry;
      const lid: Frame[] = [{ transform: T.move(0, 0), easing: EASE.sine }, { transform: T.move(sink, 0), offset: 0.5 }, { transform: T.move(sink, 0), offset: 0.64, easing: EASE.out }, { transform: T.move(0, 0), offset: 0.76 }, { transform: T.move(0, 0) }];
      return [
        this.play('lidLM', lid, timing),
        this.play('lidRM', lid, timing),
        this.play('headM', [{ transform: T.move(0, 0), easing: EASE.sine }, { transform: T.move(5, 0), offset: 0.6, easing: EASE.out }, { transform: T.move(-1, 0), offset: 0.74, easing: EASE.sine }, { transform: T.move(0, 0), offset: 0.9 }, { transform: T.move(0, 0) }], timing),
      ];
    });
  }
}

/** How each state is entered: the body first, the ears a beat later so they settle after it. */
const TIMINGS: Record<Exclude<FamiliarState, 'waking'>, { base: Timing; ears: Timing; eyes?: Timing; head?: Timing; look?: Timing; lids?: Timing; mouth?: Timing }> = {
  idle: { base: { duration: 480, easing: EASE.inOut }, ears: { duration: 760, easing: EASE.springSoft, delay: 60 } },
  thinking: { base: { duration: 520, easing: EASE.inOut }, ears: { duration: 760, easing: EASE.springSoft, delay: 90 }, look: { duration: 420, easing: EASE.inOut }, mouth: { duration: 420, easing: EASE.inOut, delay: 60 } },
  happy: { base: { duration: 380, easing: EASE.inOut }, ears: { duration: 700, easing: EASE.spring, delay: 60 }, eyes: { duration: 260, easing: EASE.inOut }, mouth: { duration: 560, easing: EASE.spring } },
  attention: { base: { duration: 300, easing: EASE.snap }, ears: { duration: 560, easing: EASE.spring, delay: 70 }, eyes: { duration: 500, easing: EASE.spring, delay: 40 }, lids: { duration: 160, easing: EASE.out }, mouth: { duration: 500, easing: EASE.spring, delay: 30 } },
  sleeping: { base: { duration: 1100, easing: EASE.sine }, ears: { duration: 1300, easing: EASE.sine, delay: 240 }, eyes: { duration: 1000, easing: EASE.sine, delay: 150 }, head: { duration: 1300, easing: EASE.sine }, mouth: { duration: 1000, easing: EASE.sine, delay: 200 } },
  sad: { base: { duration: 900, easing: EASE.inOut }, ears: { duration: 1100, easing: EASE.springSoft, delay: 160 }, head: { duration: 1000, easing: EASE.inOut }, lids: { duration: 700, easing: EASE.inOut }, mouth: { duration: 800, easing: EASE.inOut, delay: 120 } },
  offline: { base: { duration: 900, easing: EASE.sine }, ears: { duration: 1100, easing: EASE.sine, delay: 150 }, lids: { duration: 800, easing: EASE.sine }, mouth: { duration: 800, easing: EASE.sine } },
};
