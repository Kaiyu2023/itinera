import type { LngLat, LngLatBounds, MapMarker, MapRenderer, MapRoute } from './MapRenderer';
import { drawBasemap } from './mockBasemap';
import './map.css';

/**
 * MockMapRenderer — $0, keyless, offline map. Draws stylised placeholder
 * tiles on a canvas (see mockBasemap.ts), routes as SVG polylines, markers as
 * DOM nodes. Supports fit-to-bounds, drag-pan, and button zoom — enough to
 * exercise every MapRenderer call-site before GoogleMapRenderer exists.
 */
export class MockMapRenderer implements MapRenderer {
  private host: HTMLElement | null = null;
  private canvas!: HTMLCanvasElement;
  private svg!: SVGSVGElement;
  private markerLayer!: HTMLDivElement;

  private markers: MapMarker[] = [];
  private routes: MapRoute[] = [];

  private lastFit: { bounds: LngLatBounds; padding: number } | null = null;
  private center: LngLat | null = null;
  private baseScale = 0; // px per degree of latitude, at zoom 1
  private zoom = 1;
  private cosLat = 1;
  private size = { w: 0, h: 0 };

  private markerHandler: ((id: string) => void) | null = null;
  private mapHandler: (() => void) | null = null;
  private viewHandler: (() => void) | null = null;

  private resizeObserver: ResizeObserver | null = null;
  private drag: { x: number; y: number; moved: boolean } | null = null;
  private rafId = 0;

  mount(container: HTMLElement): void {
    this.host = container;
    container.classList.add('mmr');

    this.canvas = document.createElement('canvas');
    this.svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    this.svg.setAttribute('class', 'mmr-routes');
    this.markerLayer = document.createElement('div');
    this.markerLayer.className = 'mmr-markers';
    container.append(this.canvas, this.svg, this.markerLayer);

    const zoomCtl = document.createElement('div');
    zoomCtl.className = 'mmr-zoom mmr-ctl';
    const zin = document.createElement('button');
    zin.type = 'button';
    zin.textContent = '+';
    zin.setAttribute('aria-label', 'Zoom in');
    zin.addEventListener('click', () => this.zoomBy(1.5));
    const zout = document.createElement('button');
    zout.type = 'button';
    zout.textContent = '−';
    zout.setAttribute('aria-label', 'Zoom out');
    zout.addEventListener('click', () => this.zoomBy(1 / 1.5));
    zoomCtl.append(zin, zout);

    const attribution = document.createElement('div');
    attribution.className = 'mmr-attribution';
    attribution.textContent =
      'MockMapRenderer — stylised placeholder tiles · swaps to GoogleMapRenderer via the MapRenderer interface';
    container.append(zoomCtl, attribution);

    container.addEventListener('pointerdown', this.onPointerDown);
    container.addEventListener('pointermove', this.onPointerMove);
    container.addEventListener('pointerup', this.onPointerUp);
    container.addEventListener('pointercancel', this.onPointerCancel);

    this.resizeObserver = new ResizeObserver(() => this.handleResize());
    this.resizeObserver.observe(container);
  }

  destroy(): void {
    const host = this.host;
    if (!host) return;
    this.resizeObserver?.disconnect();
    host.removeEventListener('pointerdown', this.onPointerDown);
    host.removeEventListener('pointermove', this.onPointerMove);
    host.removeEventListener('pointerup', this.onPointerUp);
    host.removeEventListener('pointercancel', this.onPointerCancel);
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

  fitBounds(bounds: LngLatBounds, padding = 24): void {
    this.lastFit = { bounds, padding };
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
    const { bounds, padding } = this.lastFit;
    const midLat = (bounds.north + bounds.south) / 2;
    this.cosLat = Math.max(0.2, Math.cos((midLat * Math.PI) / 180));
    const spanLng = Math.max(1e-6, bounds.east - bounds.west);
    const spanLat = Math.max(1e-6, bounds.north - bounds.south);
    const usableW = Math.max(40, w - 2 * padding);
    const usableH = Math.max(40, h - 2 * padding);
    this.baseScale = Math.min(usableW / (spanLng * this.cosLat), usableH / spanLat);
    this.center = { lng: (bounds.west + bounds.east) / 2, lat: midLat };
  }

  private get scale() {
    return this.baseScale * this.zoom;
  }

  private projectXY(lng: number, lat: number): [number, number] {
    const c = this.center!;
    return [this.size.w / 2 + (lng - c.lng) * this.cosLat * this.scale, this.size.h / 2 + (c.lat - lat) * this.scale];
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

  /* ── drawing ────────────────────────────────────────────────────── */

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
      drawBasemap(ctx, (lng, lat) => this.projectXY(lng, lat), w, h, this.center, lngSpan);
    }
    this.renderRoutes();
    this.renderMarkers();
  }

  private renderRoutes() {
    if (!this.center || !this.size.w) return;
    this.svg.setAttribute('width', String(this.size.w));
    this.svg.setAttribute('height', String(this.size.h));
    this.svg.replaceChildren();
    for (const route of this.routes) {
      if (route.points.length < 2) continue;
      const d = smoothPath(route.points.map((p) => this.projectXY(p.lng, p.lat)));
      const line = document.createElementNS('http://www.w3.org/2000/svg', 'path');
      line.setAttribute('d', d);
      line.setAttribute('fill', 'none');
      line.setAttribute('stroke', route.color);
      line.setAttribute('stroke-width', '4');
      line.setAttribute('stroke-linecap', 'round');
      line.setAttribute('stroke-linejoin', 'round');
      line.setAttribute('stroke-opacity', '0.95');
      if (route.dashed) line.setAttribute('stroke-dasharray', '2 7');
      const halo = line.cloneNode() as SVGPathElement;
      halo.setAttribute('stroke', '#ffffff');
      halo.setAttribute('stroke-width', '7.5');
      halo.setAttribute('stroke-opacity', '0.85');
      halo.removeAttribute('stroke-dasharray');
      this.svg.append(halo, line);
    }
  }

  private renderMarkers() {
    if (!this.center || !this.size.w) return;
    this.markerLayer.replaceChildren();
    for (const m of this.markers) {
      const [x, y] = this.projectXY(m.position.lng, m.position.lat);
      const el = document.createElement('div');
      const clickable = m.interactive !== false;
      el.className = `mmr-mk mmr-${m.variant}${m.selected ? ' sel' : ''}${clickable ? ' clickable' : ''}`;
      el.dataset.id = m.id;
      el.style.left = `${x}px`;
      el.style.top = `${y}px`;
      if (m.color) el.style.setProperty('--kc', m.color);
      if (m.variant !== 'chip') {
        const pin = document.createElement('div');
        pin.className = 'mmr-pin';
        if (m.label) pin.textContent = m.label;
        el.append(pin);
      }
      if (m.tag) {
        const tag = document.createElement('span');
        tag.className = `mmr-tag${m.tagPlacement === 'above' ? ' mmr-tag-above' : m.tagPlacement === 'left' ? ' mmr-tag-left' : ''}`;
        tag.textContent = m.tag;
        el.append(tag);
      }
      this.markerLayer.append(el);
    }
  }
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
