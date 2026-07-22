/**
 * Stylised placeholder basemaps for MockMapRenderer. Hand-drawn, keyless,
 * offline. Each region sketch is deliberately impressionistic — enough real
 * geography (coasts, rivers, ridges, rail) to orient a traveller, drawn from
 * a few dozen real coordinates rather than tiles. A real tile provider
 * replaces all of this via the MapRenderer interface, not by editing it.
 */

import type { LngLat } from './MapRenderer';

export type Proj = (lng: number, lat: number) => [number, number];

const PAPER = '#f4f2ea';
const WATER = '#d5e2ec';
const RIVER = '#c9dcea';
const HILL = '#e5ebd8';
const RIDGE = '#e9ecdd';
const GRID = '#e6e2d6';
const ROAD = '#ffffff';
const RAIL = '#b8b2a4';
const COAST = '#b7c9d6';
const LABEL = '#a09a8a';
const WATER_LABEL = '#8fa8bb';
const CITY_LABEL = '#55524a';

type Pt = [number, number]; // [lng, lat]

function tracePath(ctx: CanvasRenderingContext2D, P: Proj, pts: Pt[]) {
  pts.forEach((p, i) => {
    const [x, y] = P(p[0], p[1]);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
}

function fillPoly(ctx: CanvasRenderingContext2D, P: Proj, pts: Pt[], color: string) {
  ctx.fillStyle = color;
  ctx.beginPath();
  tracePath(ctx, P, pts);
  ctx.closePath();
  ctx.fill();
}

function strokeLine(ctx: CanvasRenderingContext2D, P: Proj, pts: Pt[], color: string, width: number, dash?: number[]) {
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  if (dash) ctx.setLineDash(dash);
  ctx.beginPath();
  tracePath(ctx, P, pts);
  ctx.stroke();
  if (dash) ctx.setLineDash([]);
}

/** Quadratic-midpoint smoothing, same curve family the route lines use. */
function traceSmooth(ctx: CanvasRenderingContext2D, P: Proj, pts: Pt[]) {
  const first = P(pts[0][0], pts[0][1]);
  ctx.moveTo(first[0], first[1]);
  for (let i = 1; i < pts.length - 1; i++) {
    const [cx, cy] = P(pts[i][0], pts[i][1]);
    const [nx, ny] = P(pts[i + 1][0], pts[i + 1][1]);
    ctx.quadraticCurveTo(cx, cy, (cx + nx) / 2, (cy + ny) / 2);
  }
  const last = P(pts[pts.length - 1][0], pts[pts.length - 1][1]);
  ctx.lineTo(last[0], last[1]);
}

type LabelKind = 'area' | 'water' | 'city' | 'small';

function label(
  ctx: CanvasRenderingContext2D,
  P: Proj,
  lng: number,
  lat: number,
  text: string,
  kind: LabelKind = 'area',
) {
  const [x, y] = P(lng, lat);
  ctx.save();
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  const spaced = ctx as CanvasRenderingContext2D & { letterSpacing?: string };
  if (kind === 'area') {
    ctx.font = '700 10px Inter, system-ui, sans-serif';
    ctx.fillStyle = LABEL;
    if ('letterSpacing' in spaced) spaced.letterSpacing = '1.4px';
    text = text.toUpperCase();
  } else if (kind === 'water') {
    ctx.font = 'italic 600 10.5px Inter, system-ui, sans-serif';
    ctx.fillStyle = WATER_LABEL;
  } else if (kind === 'city') {
    ctx.font = "650 13px 'Bricolage Grotesque', Inter, system-ui, sans-serif";
    ctx.fillStyle = CITY_LABEL;
  } else {
    ctx.font = '600 11px Inter, system-ui, sans-serif';
    ctx.fillStyle = LABEL;
  }
  ctx.fillText(text, x, y);
  ctx.restore();
}

function streetGrid(
  ctx: CanvasRenderingContext2D,
  P: Proj,
  lng0: number,
  lng1: number,
  lngStep: number,
  lat0: number,
  lat1: number,
  latStep: number,
) {
  ctx.strokeStyle = GRID;
  ctx.lineWidth = 1.2;
  for (let lng = lng0; lng <= lng1; lng += lngStep) {
    strokeLine(
      ctx,
      P,
      [
        [lng, lat1],
        [lng, lat0],
      ],
      GRID,
      1.2,
    );
  }
  for (let lat = lat0; lat <= lat1; lat += latStep) {
    strokeLine(
      ctx,
      P,
      [
        [lng0, lat],
        [lng1, lat],
      ],
      GRID,
      1.2,
    );
  }
}

function ridgeBlobs(ctx: CanvasRenderingContext2D, P: Proj, blobs: [number, number, number][]) {
  ctx.fillStyle = RIDGE;
  for (const [lng, lat, r] of blobs) {
    const [x, y] = P(lng, lat);
    ctx.beginPath();
    ctx.ellipse(x, y, r, r * 0.55, 0, 0, 7);
    ctx.fill();
  }
}

/* ── Kyoto basin ─────────────────────────────────────────────────────── */

function drawKyoto(ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) {
  ctx.fillStyle = PAPER;
  ctx.fillRect(0, 0, w, h);
  // hills — Higashiyama (east ridge), Arashiyama (west), Fushimi (SE)
  fillPoly(
    ctx,
    P,
    [
      [135.79, 35.11],
      [135.802, 35.03],
      [135.797, 34.995],
      [135.806, 34.955],
      [135.86, 34.94],
      [135.86, 35.11],
    ],
    HILL,
  );
  fillPoly(
    ctx,
    P,
    [
      [135.63, 35.11],
      [135.676, 35.05],
      [135.668, 35.02],
      [135.66, 34.995],
      [135.63, 34.98],
    ],
    HILL,
  );
  fillPoly(
    ctx,
    P,
    [
      [135.777, 34.972],
      [135.797, 34.967],
      [135.8, 34.947],
      [135.775, 34.95],
    ],
    HILL,
  );
  // Kyoto Gyoen (Imperial Palace park)
  fillPoly(
    ctx,
    P,
    [
      [135.759, 35.029],
      [135.769, 35.029],
      [135.769, 35.017],
      [135.759, 35.017],
    ],
    HILL,
  );
  // street grid (central Kyoto really is a grid)
  streetGrid(ctx, P, 135.712, 135.792, 0.006, 34.972, 35.036, 0.005);
  // major roads
  ctx.lineCap = 'round';
  strokeLine(
    ctx,
    P,
    [
      [135.7513, 35.06],
      [135.7513, 34.955],
    ],
    ROAD,
    4,
  );
  strokeLine(
    ctx,
    P,
    [
      [135.759, 35.055],
      [135.759, 34.95],
    ],
    ROAD,
    4,
  );
  strokeLine(
    ctx,
    P,
    [
      [135.66, 35.0037],
      [135.8, 35.0037],
    ],
    ROAD,
    4,
  );
  strokeLine(
    ctx,
    P,
    [
      [135.68, 34.9949],
      [135.8, 34.9949],
    ],
    ROAD,
    4,
  );
  // rivers — Kamo, Katsura
  strokeLine(
    ctx,
    P,
    [
      [135.7737, 35.11],
      [135.7716, 35.02],
      [135.7719, 34.995],
      [135.768, 34.968],
      [135.764, 34.93],
    ],
    RIVER,
    7,
  );
  strokeLine(
    ctx,
    P,
    [
      [135.655, 35.035],
      [135.678, 35.011],
      [135.7, 34.988],
      [135.72, 34.952],
      [135.729, 34.93],
    ],
    RIVER,
    8,
  );
  // rail through Kyoto Station
  strokeLine(
    ctx,
    P,
    [
      [135.63, 35.004],
      [135.74, 34.99],
      [135.7588, 34.9858],
      [135.83, 34.992],
    ],
    RAIL,
    2,
    [7, 5],
  );
  label(ctx, P, 135.789, 35.02, 'Higashiyama');
  label(ctx, P, 135.678, 35.038, 'Arashiyama');
  label(ctx, P, 135.779, 35.0025, 'Gion');
  label(ctx, P, 135.779, 34.958, 'Fushimi');
  label(ctx, P, 135.7588, 34.9838, 'Kyoto Station', 'small');
  label(ctx, P, 135.7745, 35.042, 'Kamo River', 'water');
}

/* ── Tokyo ───────────────────────────────────────────────────────────── */

function drawTokyo(ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) {
  ctx.fillStyle = PAPER;
  ctx.fillRect(0, 0, w, h);
  // Tokyo Bay — western/northern shore traced, closed through open water
  ctx.fillStyle = WATER;
  ctx.beginPath();
  traceSmooth(ctx, P, [
    [139.62, 35.3],
    [139.67, 35.38],
    [139.71, 35.44],
    [139.745, 35.5],
    [139.755, 35.53],
    [139.765, 35.555],
    [139.748, 35.575],
    [139.752, 35.6],
    [139.765, 35.615],
    [139.78, 35.622],
    [139.792, 35.632],
    [139.795, 35.648],
    [139.782, 35.662],
    [139.802, 35.673],
    [139.83, 35.663],
    [139.87, 35.672],
    [139.905, 35.66],
    [139.94, 35.62],
    [139.95, 35.52],
    [139.9, 35.4],
    [139.82, 35.28],
  ]);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = COAST;
  ctx.lineWidth = 1.5;
  ctx.stroke();
  // street grid, light — Tokyo is not a grid, but the abstraction reads "city"
  streetGrid(ctx, P, 139.66, 139.82, 0.008, 35.62, 35.74, 0.0065);
  // parks: Yoyogi/Meiji forest, Imperial Palace, Shinjuku Gyoen, Ueno
  fillPoly(
    ctx,
    P,
    [
      [139.688, 35.678],
      [139.703, 35.679],
      [139.706, 35.669],
      [139.697, 35.662],
      [139.686, 35.666],
    ],
    HILL,
  );
  fillPoly(
    ctx,
    P,
    [
      [139.744, 35.692],
      [139.76, 35.693],
      [139.76, 35.68],
      [139.746, 35.678],
    ],
    HILL,
  );
  fillPoly(
    ctx,
    P,
    [
      [139.706, 35.689],
      [139.716, 35.688],
      [139.714, 35.681],
      [139.704, 35.683],
    ],
    HILL,
  );
  fillPoly(
    ctx,
    P,
    [
      [139.766, 35.72],
      [139.776, 35.722],
      [139.778, 35.712],
      [139.768, 35.71],
    ],
    HILL,
  );
  // Sumida river down past Asakusa to the bay
  strokeLine(
    ctx,
    P,
    [
      [139.8, 35.73],
      [139.797, 35.71],
      [139.8, 35.698],
      [139.786, 35.68],
      [139.786, 35.665],
      [139.772, 35.648],
    ],
    RIVER,
    6,
  );
  // Yamanote loop (dashed)
  {
    const [cx, cy] = P(139.7375, 35.685);
    const [ex] = P(139.7375 + 0.042, 35.685);
    const [, ey] = P(139.7375, 35.685 + 0.037);
    ctx.setLineDash([7, 5]);
    ctx.strokeStyle = RAIL;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.ellipse(cx, cy, Math.abs(ex - cx), Math.abs(ey - cy), 0, 0, 7);
    ctx.stroke();
    ctx.setLineDash([]);
  }
  label(ctx, P, 139.692, 35.697, 'Shinjuku');
  label(ctx, P, 139.699, 35.6555, 'Shibuya');
  label(ctx, P, 139.796, 35.7185, 'Asakusa');
  label(ctx, P, 139.766, 35.6705, 'Ginza');
  label(ctx, P, 139.84, 35.59, 'Tokyo Bay', 'water');
}

/* ── Hakone ──────────────────────────────────────────────────────────── */

function drawHakone(ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) {
  ctx.fillStyle = PAPER;
  ctx.fillRect(0, 0, w, h);
  ridgeBlobs(ctx, P, [
    [139.02, 35.27, 70],
    [139.06, 35.26, 55],
    [138.99, 35.22, 80],
    [139.045, 35.2, 60],
    [139.1, 35.185, 55],
    [139.13, 35.26, 50],
    [138.98, 35.28, 60],
    [139.075, 35.29, 45],
  ]);
  // Sagami Bay, SE corner
  ctx.fillStyle = WATER;
  ctx.beginPath();
  traceSmooth(ctx, P, [
    [139.12, 35.13],
    [139.16, 35.16],
    [139.21, 35.19],
    [139.26, 35.24],
    [139.32, 35.27],
    [139.34, 35.13],
  ]);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = COAST;
  ctx.lineWidth = 1.5;
  ctx.stroke();
  // Lake Ashi
  ctx.fillStyle = WATER;
  ctx.beginPath();
  traceSmooth(ctx, P, [
    [139.007, 35.242],
    [139.003, 35.228],
    [139.008, 35.213],
    [139.017, 35.198],
    [139.029, 35.19],
    [139.034, 35.203],
    [139.028, 35.221],
    [139.018, 35.234],
    [139.01, 35.244],
  ]);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = COAST;
  ctx.stroke();
  // Tozan railway switchbacks + ropeway
  strokeLine(
    ctx,
    P,
    [
      [139.155, 35.256],
      [139.135, 35.245],
      [139.107, 35.232],
      [139.093, 35.226],
      [139.082, 35.233],
      [139.068, 35.24],
      [139.052, 35.246],
    ],
    RAIL,
    2,
    [7, 5],
  );
  strokeLine(
    ctx,
    P,
    [
      [139.048, 35.247],
      [139.028, 35.243],
      [139.012, 35.238],
    ],
    RAIL,
    1.5,
    [3, 4],
  );
  label(ctx, P, 139.018, 35.211, 'Lake Ashi', 'water');
  label(ctx, P, 139.24, 35.2, 'Sagami Bay', 'water');
  label(ctx, P, 139.03, 35.253, 'Sengokuhara');
}

/* ── Osaka ───────────────────────────────────────────────────────────── */

function drawOsaka(ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) {
  ctx.fillStyle = PAPER;
  ctx.fillRect(0, 0, w, h);
  // Osaka Bay along the west, down to the KIX shore
  ctx.fillStyle = WATER;
  ctx.beginPath();
  traceSmooth(ctx, P, [
    [135.45, 34.79],
    [135.425, 34.72],
    [135.437, 34.7],
    [135.442, 34.685],
    [135.428, 34.66],
    [135.405, 34.645],
    [135.378, 34.62],
    [135.348, 34.6],
    [135.328, 34.57],
    [135.3, 34.53],
    [135.272, 34.48],
    [135.252, 34.44],
    [135.246, 34.4],
    [135.26, 34.355],
    [135.1, 34.3],
    [135.05, 34.8],
  ]);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = COAST;
  ctx.lineWidth = 1.5;
  ctx.stroke();
  // KIX — reclaimed island + access bridge
  fillPoly(
    ctx,
    P,
    [
      [135.222, 34.437],
      [135.246, 34.447],
      [135.266, 34.433],
      [135.242, 34.423],
    ],
    PAPER,
  );
  strokeLine(
    ctx,
    P,
    [
      [135.255, 34.438],
      [135.31, 34.457],
    ],
    RAIL,
    2,
    [5, 4],
  );
  // street grid
  streetGrid(ctx, P, 135.455, 135.545, 0.006, 34.625, 34.725, 0.005);
  // Yodo river + Dōtonbori canal
  strokeLine(
    ctx,
    P,
    [
      [135.565, 34.745],
      [135.52, 34.722],
      [135.49, 34.703],
      [135.457, 34.692],
      [135.44, 34.686],
    ],
    RIVER,
    7,
  );
  strokeLine(
    ctx,
    P,
    [
      [135.478, 34.6693],
      [135.522, 34.6688],
    ],
    RIVER,
    3,
  );
  // Osaka Castle park
  fillPoly(
    ctx,
    P,
    [
      [135.519, 34.693],
      [135.533, 34.693],
      [135.533, 34.681],
      [135.519, 34.681],
    ],
    HILL,
  );
  label(ctx, P, 135.498, 34.706, 'Umeda');
  label(ctx, P, 135.501, 34.66, 'Namba');
  label(ctx, P, 135.34, 34.55, 'Osaka Bay', 'water');
}

/* ── Kansai → Kanto overview (the trip view) ─────────────────────────── */

function drawJapan(ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) {
  ctx.fillStyle = WATER;
  ctx.fillRect(0, 0, w, h); // ocean everywhere…
  // …then land as one polygon: Pacific coast west→east, closed along the top
  const coast: Pt[] = [
    [134.9, 34.75],
    [135.1, 34.7],
    [135.28, 34.72],
    [135.42, 34.68],
    [135.46, 34.55],
    [135.4, 34.42],
    [135.26, 34.32],
    [135.1, 34.24],
    [135.06, 33.9],
    [135.3, 33.55],
    [135.75, 33.44],
    [136.1, 33.7],
    [136.3, 34.1],
    [136.5, 34.42],
    [136.72, 34.5],
    [136.6, 34.62],
    [136.55, 34.85],
    [136.65, 35.02],
    [136.85, 35.07],
    [136.95, 34.9],
    [136.98, 34.68],
    [137.15, 34.75],
    [137.32, 34.64],
    [137.6, 34.63],
    [138.0, 34.6],
    [138.25, 34.65],
    [138.38, 34.95],
    [138.5, 35.02],
    [138.55, 34.92],
    [138.6, 34.66],
    [138.75, 34.6],
    [138.9, 34.6],
    [138.98, 34.72],
    [139.08, 34.92],
    [139.15, 35.1],
    [139.25, 35.2],
    [139.4, 35.32],
    [139.55, 35.33],
    [139.62, 35.2],
    [139.68, 35.14],
    [139.72, 35.22],
    [139.78, 35.32],
    [139.82, 35.46],
    [139.9, 35.6],
    [140.0, 35.66],
    [140.08, 35.56],
    [140.05, 35.4],
    [139.98, 35.32],
    [139.95, 35.2],
    [140.02, 35.08],
    [140.1, 34.95],
  ];
  ctx.fillStyle = PAPER;
  ctx.beginPath();
  traceSmooth(ctx, P, coast);
  const last = P(coast[coast.length - 1][0], coast[coast.length - 1][1]);
  ctx.lineTo(w + 40, last[1]);
  ctx.lineTo(w + 40, -40);
  ctx.lineTo(-40, -40);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = COAST;
  ctx.lineWidth = 1.5;
  ctx.stroke();
  // Lake Biwa
  fillPoly(
    ctx,
    P,
    [
      [136.07, 35.02],
      [136.0, 35.18],
      [136.06, 35.38],
      [136.18, 35.44],
      [136.28, 35.3],
      [136.21, 35.1],
    ],
    WATER,
  );
  // mountain ridges
  ridgeBlobs(ctx, P, [
    [135.8, 35.65, 90],
    [136.6, 35.75, 110],
    [137.5, 35.6, 130],
    [138.25, 35.75, 90],
    [139.0, 35.75, 70],
    [135.95, 34.5, 55],
    [137.2, 35.1, 80],
  ]);
  // Mt Fuji
  {
    const [fx, fy] = P(138.7274, 35.3606);
    ctx.fillStyle = '#d8dbe2';
    ctx.beginPath();
    ctx.moveTo(fx - 16, fy + 10);
    ctx.lineTo(fx, fy - 14);
    ctx.lineTo(fx + 16, fy + 10);
    ctx.closePath();
    ctx.fill();
    ctx.fillStyle = '#ffffff';
    ctx.beginPath();
    ctx.moveTo(fx - 6, fy - 5);
    ctx.lineTo(fx, fy - 14);
    ctx.lineTo(fx + 6, fy - 5);
    ctx.closePath();
    ctx.fill();
  }
  // Tōkaidō shinkansen (dashed)
  strokeLine(
    ctx,
    P,
    [
      [139.77, 35.68],
      [139.62, 35.47],
      [139.16, 35.26],
      [138.62, 34.98],
      [137.98, 34.77],
      [137.38, 34.77],
      [136.88, 35.1],
      [136.3, 35.05],
      [135.94, 34.99],
      [135.7588, 34.9858],
      [135.5, 34.7],
      [135.36, 34.66],
    ],
    RAIL,
    2,
    [8, 6],
  );
  // Nagoya — basemap furniture, not a trip city
  {
    const [nx, ny] = P(136.9, 35.15);
    ctx.fillStyle = CITY_LABEL;
    ctx.beginPath();
    ctx.arc(nx, ny, 3, 0, 7);
    ctx.fill();
  }
  label(ctx, P, 136.9, 35.175, 'Nagoya', 'city');
  label(ctx, P, 138.6, 35.46, 'Mt Fuji', 'small');
  label(ctx, P, 138.0, 34.35, 'Pacific Ocean', 'water');
  label(ctx, P, 136.14, 35.23, 'Lake Biwa', 'water');
}

/* ── generic fallback ────────────────────────────────────────────────── */

function drawGeneric(center: LngLat, lngSpan: number) {
  return (ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) => {
    ctx.fillStyle = PAPER;
    ctx.fillRect(0, 0, w, h);
    const step = lngSpan / 14;
    streetGrid(
      ctx,
      P,
      center.lng - lngSpan,
      center.lng + lngSpan,
      step,
      center.lat - lngSpan,
      center.lat + lngSpan,
      step * 0.8,
    );
    // a few deterministic hill blobs seeded from the centre coordinate
    const seed = Math.abs(Math.sin(center.lng * 12.9898 + center.lat * 78.233));
    const blobs: [number, number, number][] = [];
    for (let i = 0; i < 5; i++) {
      const t = (seed * (i + 1) * 43758.5453) % 1;
      const u = (seed * (i + 2) * 24634.6345) % 1;
      blobs.push([center.lng + (t - 0.5) * lngSpan * 1.4, center.lat + (u - 0.5) * lngSpan, 40 + t * 60]);
    }
    ridgeBlobs(ctx, P, blobs);
  };
}

/* ── region picker ───────────────────────────────────────────────────── */

const REGIONS: { anchor: LngLat; draw: (ctx: CanvasRenderingContext2D, P: Proj, w: number, h: number) => void }[] = [
  { anchor: { lng: 135.76, lat: 35.0 }, draw: drawKyoto },
  { anchor: { lng: 139.74, lat: 35.66 }, draw: drawTokyo },
  { anchor: { lng: 139.06, lat: 35.22 }, draw: drawHakone },
  { anchor: { lng: 135.47, lat: 34.62 }, draw: drawOsaka },
];

/** Pick and draw the basemap for the current view. */
export function drawBasemap(
  ctx: CanvasRenderingContext2D,
  P: Proj,
  w: number,
  h: number,
  center: LngLat,
  lngSpan: number,
) {
  if (lngSpan > 1.2) {
    drawJapan(ctx, P, w, h);
    return;
  }
  let best: (typeof REGIONS)[number] | null = null;
  let bestD = 0.6; // degrees — beyond this, fall back to generic
  for (const r of REGIONS) {
    const d = Math.hypot(r.anchor.lng - center.lng, r.anchor.lat - center.lat);
    if (d < bestD) {
      best = r;
      bestD = d;
    }
  }
  (best ? best.draw : drawGeneric(center, lngSpan))(ctx, P, w, h);
}
