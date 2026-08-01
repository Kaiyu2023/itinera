import type { EdgePadPx, LngLat, LngLatBounds, MapMarker, MapRenderer, MapRoute, MapUiLabels } from './MapRenderer';
import { drawBasemap, FALLBACK_BASEMAP } from './mockBasemap';
import type { BasemapPalette } from './mockBasemap';
import './map.css';

const SVG_NS = 'http://www.w3.org/2000/svg';

/** Two pins closer than this on screen are the same dot to a finger. */
const SPIDER_PX = 30;

/** Where a tag may sit relative to its pin, and who gets to keep one first. */
type Placement = 'below' | 'above' | 'right' | 'left';
interface Rect {
  left: number;
  top: number;
  w: number;
  h: number;
}

/**
 * MockMapRenderer — $0, keyless, offline map. Draws stylised placeholder
 * tiles on a canvas (see mockBasemap.ts), routes as SVG polylines, markers as
 * DOM nodes. Supports fit-to-bounds, drag-pan, and button zoom — enough to
 * exercise every MapRenderer call-site before GoogleMapRenderer exists.
 *
 * Two responsibilities live here and cannot live above: the palette (canvas
 * has no cascade, so the renderer reads the CSS custom properties itself) and
 * everything that depends on *screen* positions — pin spiderfy and tag
 * declutter. Both were previously attempted in geographic degrees a layer up,
 * which is why three stops within 400 m of each other shipped three pins on
 * one pixel with the wrong name under them.
 */
export class MockMapRenderer implements MapRenderer {
  private host: HTMLElement | null = null;
  private canvas!: HTMLCanvasElement;
  private svg!: SVGSVGElement;
  /** Leader lines from a spiderfied pin back to its true position. */
  private spider!: SVGSVGElement;
  private markerLayer!: HTMLDivElement;
  private zoomInButton!: HTMLButtonElement;
  private zoomOutButton!: HTMLButtonElement;
  private attribution!: HTMLDivElement;

  private markers: MapMarker[] = [];
  private routes: MapRoute[] = [];

  private lastFit: { bounds: LngLatBounds; pad: EdgePadPx } | null = null;
  private center: LngLat | null = null;
  /** Container pixel that `center` projects to — the middle of the *usable*
      rect, which is not the middle of the container once a bottom sheet and a
      floating toolbar have taken their edges. */
  private origin = { x: 0, y: 0 };
  private baseScale = 0; // px per degree of latitude, at zoom 1
  private zoom = 1;
  private cosLat = 1;
  private size = { w: 0, h: 0 };
  private palette: BasemapPalette | null = null;

  private markerHandler: ((id: string) => void) | null = null;
  private mapHandler: (() => void) | null = null;
  private viewHandler: (() => void) | null = null;

  private resizeObserver: ResizeObserver | null = null;
  private themeObserver: MutationObserver | null = null;
  private drag: { x: number; y: number; moved: boolean } | null = null;
  private rafId = 0;
  private uiLabels: MapUiLabels;

  constructor(uiLabels: MapUiLabels) {
    this.uiLabels = uiLabels;
  }

  mount(container: HTMLElement): void {
    this.host = container;
    container.classList.add('mmr');

    this.canvas = document.createElement('canvas');
    this.svg = document.createElementNS(SVG_NS, 'svg');
    this.svg.setAttribute('class', 'mmr-routes');
    this.spider = document.createElementNS(SVG_NS, 'svg');
    this.spider.setAttribute('class', 'mmr-spider');
    this.markerLayer = document.createElement('div');
    this.markerLayer.className = 'mmr-markers';
    container.append(this.canvas, this.svg, this.spider, this.markerLayer);

    const zoomCtl = document.createElement('div');
    zoomCtl.className = 'mmr-zoom mmr-ctl';
    this.zoomInButton = document.createElement('button');
    this.zoomInButton.type = 'button';
    this.zoomInButton.textContent = '+';
    this.zoomInButton.addEventListener('click', () => this.zoomBy(1.5));
    this.zoomOutButton = document.createElement('button');
    this.zoomOutButton.type = 'button';
    this.zoomOutButton.textContent = '−';
    this.zoomOutButton.addEventListener('click', () => this.zoomBy(1 / 1.5));
    zoomCtl.append(this.zoomInButton, this.zoomOutButton);

    this.attribution = document.createElement('div');
    this.attribution.className = 'mmr-attribution';
    this.applyUiLabels();
    container.append(zoomCtl, this.attribution);

    container.addEventListener('pointerdown', this.onPointerDown);
    container.addEventListener('pointermove', this.onPointerMove);
    container.addEventListener('pointerup', this.onPointerUp);
    container.addEventListener('pointercancel', this.onPointerCancel);
    this.markerLayer.addEventListener('keydown', this.onMarkerKey);

    this.resizeObserver = new ResizeObserver(() => this.handleResize());
    this.resizeObserver.observe(container);

    // The basemap is canvas, so it cannot inherit a theme change the way the
    // DOM chrome does — it has to be redrawn by hand. The app resolves the
    // theme onto `<html data-theme>` (see index.html), so that attribute, and
    // not the media query, is the thing to watch.
    this.themeObserver = new MutationObserver(this.onThemeChange);
    this.themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  }

  destroy(): void {
    const host = this.host;
    if (!host) return;
    this.resizeObserver?.disconnect();
    this.themeObserver?.disconnect();
    host.removeEventListener('pointerdown', this.onPointerDown);
    host.removeEventListener('pointermove', this.onPointerMove);
    host.removeEventListener('pointerup', this.onPointerUp);
    host.removeEventListener('pointercancel', this.onPointerCancel);
    this.markerLayer.removeEventListener('keydown', this.onMarkerKey);
    host.classList.remove('mmr');
    host.replaceChildren();
    this.host = null;
    cancelAnimationFrame(this.rafId);
  }

  setMarkers(markers: MapMarker[]): void {
    this.markers = markers;
    this.renderMarkers();
  }

  setRoutes(routes: MapRoute[]): void {
    this.routes = routes;
    this.renderRoutes();
  }

  setUiLabels(labels: MapUiLabels): void {
    this.uiLabels = labels;
    if (this.host) this.applyUiLabels();
  }

  private applyUiLabels(): void {
    this.zoomInButton.setAttribute('aria-label', this.uiLabels.zoomIn);
    this.zoomOutButton.setAttribute('aria-label', this.uiLabels.zoomOut);
    this.attribution.textContent = this.uiLabels.attribution;
  }

  fitBounds(bounds: LngLatBounds, padding: number | EdgePadPx = 24): void {
    const pad =
      typeof padding === 'number' ? { top: padding, right: padding, bottom: padding, left: padding } : padding;
    this.lastFit = { bounds, pad };
    this.zoom = 1;
    this.computeView();
    this.render();
    this.viewHandler?.();
  }

  project(position: LngLat): { x: number; y: number } | null {
    if (!this.center || !this.size.w) return null;
    const [x, y] = this.projectXY(position.lng, position.lat);
    return { x, y };
  }

  onMarkerClick(handler: ((id: string) => void) | null): void {
    this.markerHandler = handler;
  }

  onMapClick(handler: (() => void) | null): void {
    this.mapHandler = handler;
  }

  onViewChange(handler: (() => void) | null): void {
    this.viewHandler = handler;
  }

  /* ── view maths ─────────────────────────────────────────────────── */

  private computeView() {
    const host = this.host;
    if (!host || !this.lastFit) return;
    const w = host.clientWidth;
    const h = host.clientHeight;
    if (!w || !h) return;
    this.size = { w, h };
    const { bounds, pad } = this.lastFit;
    const midLat = (bounds.north + bounds.south) / 2;
    this.cosLat = Math.max(0.2, Math.cos((midLat * Math.PI) / 180));
    const spanLng = Math.max(1e-6, bounds.east - bounds.west);
    const spanLat = Math.max(1e-6, bounds.north - bounds.south);
    const usableW = Math.max(40, w - pad.left - pad.right);
    const usableH = Math.max(40, h - pad.top - pad.bottom);
    this.baseScale = Math.min(usableW / (spanLng * this.cosLat), usableH / spanLat);
    this.center = { lng: (bounds.west + bounds.east) / 2, lat: midLat };
    // Fit the geometry into the *visible* rectangle. Shrinking the scale alone
    // (what a scalar padding does) still centres on the container, so with a
    // 46%-tall bottom sheet the day's stops landed behind it.
    this.origin = { x: pad.left + usableW / 2, y: pad.top + usableH / 2 };
  }

  private get scale() {
    return this.baseScale * this.zoom;
  }

  private projectXY(lng: number, lat: number): [number, number] {
    const c = this.center!;
    return [this.origin.x + (lng - c.lng) * this.cosLat * this.scale, this.origin.y + (c.lat - lat) * this.scale];
  }

  private zoomBy(factor: number) {
    this.zoom = Math.min(10, Math.max(0.5, this.zoom * factor));
    this.render();
    this.viewHandler?.();
  }

  private handleResize() {
    if (!this.lastFit) return;
    this.computeView();
    this.render();
    this.viewHandler?.();
  }

  private onThemeChange = () => {
    this.palette = null;
    this.render();
  };

  /* ── interaction ────────────────────────────────────────────────── */

  private onPointerDown = (e: PointerEvent) => {
    if ((e.target as Element).closest('.mmr-ctl')) return;
    this.drag = { x: e.clientX, y: e.clientY, moved: false };
    this.host?.setPointerCapture(e.pointerId);
  };

  private onPointerMove = (e: PointerEvent) => {
    const drag = this.drag;
    if (!drag || !this.center) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (!drag.moved && Math.hypot(dx, dy) < 4) return;
    drag.moved = true;
    drag.x = e.clientX;
    drag.y = e.clientY;
    this.center = {
      lng: this.center.lng - dx / (this.scale * this.cosLat),
      lat: this.center.lat + dy / this.scale,
    };
    cancelAnimationFrame(this.rafId);
    this.rafId = requestAnimationFrame(() => {
      this.render();
      this.viewHandler?.();
    });
  };

  private onPointerUp = (e: PointerEvent) => {
    const drag = this.drag;
    this.drag = null;
    if (!drag || drag.moved) return;
    // A click, not a pan. With pointer capture the event targets the host,
    // so hit-test the actual point.
    const hit = document.elementFromPoint(e.clientX, e.clientY);
    const mk = hit?.closest<HTMLElement>('.mmr-mk');
    if (mk?.dataset.id && mk.classList.contains('clickable')) {
      this.markerHandler?.(mk.dataset.id);
    } else {
      this.mapHandler?.();
    }
  };

  private onPointerCancel = () => {
    this.drag = null;
  };

  /** Markers are real buttons now, so they answer to a keyboard. */
  private onMarkerKey = (e: KeyboardEvent) => {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    const mk = (e.target as Element | null)?.closest<HTMLElement>('.mmr-mk.clickable');
    if (!mk?.dataset.id) return;
    e.preventDefault();
    this.markerHandler?.(mk.dataset.id);
  };

  /* ── drawing ────────────────────────────────────────────────────── */

  private readPalette(): BasemapPalette {
    const host = this.host;
    if (!host) return FALLBACK_BASEMAP;
    const cs = getComputedStyle(host);
    const out = {} as BasemapPalette;
    for (const key of Object.keys(FALLBACK_BASEMAP) as (keyof BasemapPalette)[]) {
      const name = `--mmr-${key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`;
      out[key] = cs.getPropertyValue(name).trim() || FALLBACK_BASEMAP[key];
    }
    return out;
  }

  private render() {
    const host = this.host;
    if (!host || !this.center) return;
    const w = host.clientWidth;
    const h = host.clientHeight;
    if (!w || !h) return;
    this.size = { w, h };

    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = w * dpr;
    this.canvas.height = h * dpr;
    this.canvas.style.width = `${w}px`;
    this.canvas.style.height = `${h}px`;
    const ctx = this.canvas.getContext('2d');
    if (ctx) {
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const lngSpan = w / (this.scale * this.cosLat);
      this.palette ??= this.readPalette();
      drawBasemap(ctx, (lng, lat) => this.projectXY(lng, lat), w, h, this.center, lngSpan, this.palette);
    }
    this.renderRoutes();
    this.renderMarkers();
  }

  private renderRoutes() {
    if (!this.center || !this.size.w) return;
    this.svg.setAttribute('width', String(this.size.w));
    this.svg.setAttribute('height', String(this.size.h));
    this.svg.replaceChildren();
    // Every halo first, then every line. Drawn route-by-route, an opaque 7.5px
    // halo painted over the *previous* route's stroke, which is how seven day
    // routes down the same Tōkaidō corridor collapsed into whichever one was
    // drawn last, and how per-leg colouring would have notched every joint.
    const halos: SVGPathElement[] = [];
    const lines: SVGPathElement[] = [];
    for (const route of this.routes) {
      if (route.points.length < 2) continue;
      const d = smoothPath(route.points.map((p) => this.projectXY(p.lng, p.lat)));
      const width = route.width ?? 4;
      const line = document.createElementNS(SVG_NS, 'path');
      line.setAttribute('d', d);
      line.setAttribute('fill', 'none');
      line.setAttribute('stroke', route.color);
      line.setAttribute('stroke-width', String(width));
      line.setAttribute('stroke-linecap', 'round');
      line.setAttribute('stroke-linejoin', 'round');
      line.setAttribute('stroke-opacity', '0.95');
      if (route.dashed) line.setAttribute('stroke-dasharray', '2 7');
      const halo = line.cloneNode() as SVGPathElement;
      halo.setAttribute('stroke', 'var(--mmr-halo)');
      halo.setAttribute('stroke-width', String(width + 3.5));
      halo.setAttribute('stroke-opacity', '0.85');
      halo.removeAttribute('stroke-dasharray');
      halos.push(halo);
      lines.push(line);
    }
    this.svg.append(...halos, ...lines);
  }

  private renderMarkers() {
    if (!this.center || !this.size.w) return;
    // Markers are rebuilt from scratch on every pan, zoom and selection, which
    // would drop the keyboard focus ring into the void mid-navigation.
    const focusedId = this.markerLayer.contains(document.activeElement)
      ? (document.activeElement as HTMLElement).closest<HTMLElement>('.mmr-mk')?.dataset.id
      : undefined;
    this.markerLayer.replaceChildren();
    this.spider.replaceChildren();
    this.spider.setAttribute('width', String(this.size.w));
    this.spider.setAttribute('height', String(this.size.h));
    if (this.markers.length === 0) return;

    // 1 — project. Every decision below is a pixel decision.
    const placed = this.markers.map((m) => {
      const [x, y] = this.projectXY(m.position.lng, m.position.lat);
      return { m, x, y, ax: x, ay: y, el: null as HTMLElement | null, tag: null as HTMLElement | null };
    });
    type Placed = (typeof placed)[number];

    // 2 — spiderfy. Three stops inside one Shinjuku block project onto the
    // same pixel; the topmost swallowed the other two, so the second and third
    // stop of the day could not be clicked at all and the visible pin wore the
    // wrong stop's name. Overlapping pins are pushed onto a small circle and
    // joined back to their true position by a leader line, which keeps every
    // one of them addressable without pretending they are further apart than
    // they are. Chips and city dots sit this out: they are labels for a region,
    // not points you can click wrong.
    const spiderable = placed.filter((p) => p.m.variant !== 'chip' && p.m.variant !== 'city');
    const taken = new Set<Placed>();
    for (const seed of spiderable) {
      if (taken.has(seed)) continue;
      const group: Placed[] = [seed];
      taken.add(seed);
      for (let i = 0; i < group.length; i++) {
        for (const q of spiderable) {
          if (taken.has(q)) continue;
          if (Math.hypot(group[i].x - q.x, group[i].y - q.y) <= SPIDER_PX) {
            group.push(q);
            taken.add(q);
          }
        }
      }
      if (group.length < 2) continue;
      const cx = group.reduce((a, p) => a + p.x, 0) / group.length;
      const cy = group.reduce((a, p) => a + p.y, 0) / group.length;
      // Radius that keeps neighbours on the circle ~SPIDER_PX apart.
      const radius = Math.max(19, (group.length * SPIDER_PX) / (2 * Math.PI));
      group.forEach((p, i) => {
        const angle = -Math.PI / 2 + (i * 2 * Math.PI) / group.length;
        p.ax = cx + radius * Math.cos(angle);
        p.ay = cy + radius * Math.sin(angle);
        const leader = document.createElementNS(SVG_NS, 'line');
        leader.setAttribute('class', 'mmr-leader');
        leader.setAttribute('x1', String(p.x));
        leader.setAttribute('y1', String(p.y));
        leader.setAttribute('x2', String(p.ax));
        leader.setAttribute('y2', String(p.ay));
        this.spider.append(leader);
      });
    }

    // 3 — build the DOM.
    for (const p of placed) {
      const m = p.m;
      const el = document.createElement('div');
      const clickable = m.interactive !== false;
      el.className = `mmr-mk mmr-${m.variant}${m.selected ? ' sel' : ''}${clickable ? ' clickable' : ''}`;
      el.dataset.id = m.id;
      el.style.left = `${p.ax}px`;
      el.style.top = `${p.ay}px`;
      // Painter's order by screen y, so the pin lower on the map is the one in
      // front. Every marker used to share z-index 3, which handed the decision
      // to DOM order and made an occluded marker unreachable.
      el.style.zIndex = String(m.selected ? 2000 : Math.max(1, Math.round(p.ay)));
      if (m.color) el.style.setProperty('--kc', m.color);
      if (clickable) {
        el.setAttribute('role', 'button');
        el.tabIndex = 0;
        el.setAttribute('aria-label', m.ariaLabel ?? m.tag ?? m.label ?? m.id);
      }
      if (m.variant !== 'chip') {
        const pin = document.createElement('div');
        pin.className = 'mmr-pin';
        if (m.glyphPath) pin.append(glyphSvg(m.glyphPath));
        else if (m.label) pin.textContent = m.label;
        el.append(pin);
        if (m.seq != null) {
          const seq = document.createElement('span');
          seq.className = 'mmr-seq';
          seq.textContent = String(m.seq);
          el.append(seq);
        }
      }
      if (m.tag) {
        const tag = document.createElement('span');
        tag.className = 'mmr-tag';
        tag.textContent = m.tag;
        el.append(tag);
        p.tag = tag;
      }
      p.el = el;
      this.markerLayer.append(el);
      if (focusedId != null && m.id === focusedId) el.focus({ preventScroll: true });
    }

    // 4 — tag declutter, in screen space, over every variant. The old pass
    // compared stop-to-stop *degrees* and ignored home / candidate / city /
    // chip markers entirely, so the home tag rendered as "acery Shinjuku" with
    // 52px sliced off the card, and in trip view Tokyo / Days 1–3 · Tokyo and
    // HND / Ghibli Museum printed straight through each other. Now the real
    // boxes are measured after layout: a tag flips to whichever side keeps it
    // inside the frame and clear of the tags already kept, and is dropped when
    // no side works. Reads are batched ahead of the writes to keep it to one
    // reflow.
    const tagged = placed.filter((p) => p.tag);
    if (tagged.length === 0) return;
    // The marker element's own box is its pin — the tag and the seq badge are
    // both out of flow — and it is centred on the anchor.
    const pinBox = new Map<Placed, { halfW: number; halfH: number }>();
    for (const p of placed) {
      pinBox.set(p, { halfW: (p.el?.offsetWidth ?? 0) / 2, halfH: (p.el?.offsetHeight ?? 0) / 2 });
    }
    const measured = tagged.map((p) => ({
      p,
      w: p.tag!.offsetWidth,
      h: p.tag!.offsetHeight,
      ...pinBox.get(p)!,
    }));
    measured.sort((a, b) => tagRank(a.p.m) - tagRank(b.p.m));

    // Pins are obstacles as much as other tags are: a name printed across the
    // pin beside it is no more readable than one printed across another name.
    // So is the renderer's own chrome — it paints above the marker layer, so a
    // tag that lands under the zoom control simply disappears behind it.
    const kept: Rect[] = [];
    const hostBox = this.host!.getBoundingClientRect();
    const chrome = [
      ...this.host!.querySelectorAll('.mmr-ctl, .mmr-attribution'),
      // App-level overlays opt in with `data-map-chrome` (see MapRenderer).
      ...(this.host!.parentElement?.querySelectorAll('[data-map-chrome]') ?? []),
    ];
    for (const el of chrome) {
      const r = el.getBoundingClientRect();
      kept.push({ left: r.left - hostBox.left, top: r.top - hostBox.top, w: r.width, h: r.height });
    }
    for (const p of placed) {
      const { halfW, halfH } = pinBox.get(p)!;
      if (!halfW && !halfH) continue; // a chip has no pin
      const badge = p.m.seq != null ? 9 : 0; // the seq badge overhangs the shoulder
      kept.push({
        left: p.ax - halfW - 1,
        top: p.ay - halfH - 1 - badge,
        w: halfW * 2 + 2 + badge,
        h: halfH * 2 + 2 + badge,
      });
    }
    for (const t of measured) {
      let chosen: Rect | null = null;
      // Four sides at three distances. One distance is not enough in a city
      // centre: with every neighbouring pin counted as an obstacle, a tag with
      // only four candidate positions loses, and the trip overview came back
      // with no "Tokyo" on it at all. A name 30px out is still unambiguously
      // this pin's name; a missing name is not information.
      search: for (const gap of [4, 16, 30]) {
        for (const place of placementOrder(t.p.m.tagPlacement, t.p.m.variant)) {
          const rect = tagRect(place, t.p.ax, t.p.ay, t.w, t.h, t.halfW, t.halfH, gap);
          if (
            rect.left < 2 ||
            rect.top < 2 ||
            rect.left + rect.w > this.size.w - 2 ||
            rect.top + rect.h > this.size.h - 2
          )
            continue;
          if (kept.some((k) => intersects(k, rect))) continue;
          chosen = rect;
          break search;
        }
      }
      if (!chosen) {
        t.p.tag!.remove();
        continue;
      }
      kept.push(chosen);
      // Container coords → the marker element's local box.
      t.p.tag!.style.left = `${chosen.left - t.p.ax + t.halfW}px`;
      t.p.tag!.style.top = `${chosen.top - t.p.ay + t.halfH}px`;
    }
  }
}

function glyphSvg(d: string): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', 'mmr-glyph');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '2.1');
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(SVG_NS, 'path');
  path.setAttribute('d', d);
  svg.append(path);
  return svg;
}

/** Who keeps their name when two tags want the same pixels. A stop you can
    click outranks a candidate ring you cannot; a city outranks both, because at
    trip zoom the city names *are* the map — losing "Osaka" to a candidate's
    name tag tells you less, not more. */
function tagRank(m: MapMarker): number {
  if (m.selected) return 0;
  switch (m.variant) {
    case 'city':
      return 1;
    case 'stop':
    case 'bead':
      return 2;
    case 'home':
    case 'transport':
    case 'search-result':
      return 3;
    case 'chip':
      return 4;
    default:
      return 5;
  }
}

function placementOrder(preferred: MapMarker['tagPlacement'], variant: MapMarker['variant']): Placement[] {
  const rest: Placement[] = ['below', 'above', 'right', 'left'];
  // City names read as basemap type, which is set above its dot.
  const first: Placement = preferred ?? (variant === 'city' ? 'above' : 'below');
  return [first, ...rest.filter((p) => p !== first)];
}

function tagRect(
  place: Placement,
  x: number,
  y: number,
  w: number,
  h: number,
  halfW: number,
  halfH: number,
  gap: number,
): Rect {
  switch (place) {
    case 'above':
      return { left: x - w / 2, top: y - halfH - gap - h, w, h };
    case 'left':
      return { left: x - halfW - gap - w, top: y - h / 2, w, h };
    case 'right':
      return { left: x + halfW + gap, top: y - h / 2, w, h };
    default:
      return { left: x - w / 2, top: y + halfH + gap, w, h };
  }
}

function intersects(a: Rect, b: Rect): boolean {
  const pad = 2;
  return (
    a.left - pad < b.left + b.w && b.left - pad < a.left + a.w && a.top - pad < b.top + b.h && b.top - pad < a.top + a.h
  );
}

/** Polyline → gently smoothed SVG path (quadratic through midpoints). */
function smoothPath(pts: [number, number][]): string {
  if (pts.length < 3) return `M${pts.map((p) => p.join(',')).join('L')}`;
  let d = `M${pts[0][0]},${pts[0][1]}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const mx = (pts[i][0] + pts[i + 1][0]) / 2;
    const my = (pts[i][1] + pts[i + 1][1]) / 2;
    d += ` Q${pts[i][0]},${pts[i][1]} ${mx},${my}`;
  }
  d += ` L${pts[pts.length - 1][0]},${pts[pts.length - 1][1]}`;
  return d;
}
