import { createContext, useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import type { EdgePadPx, LngLat, LngLatBounds, MapMarker, MapRenderer, MapRoute } from './MapRenderer';
import { MockMapRenderer } from './MockMapRenderer';

/**
 * Renderer selection lives here and nowhere else. When GoogleMapRenderer
 * exists (Phase B), an env var / trip setting picks it — no caller changes.
 */
function createMapRenderer(): MapRenderer {
  return new MockMapRenderer();
}

interface MapProjection {
  project: (position: LngLat) => { x: number; y: number } | null;
  /** Bumped on zoom/pan/resize so overlays re-project. */
  version: number;
}

const MapProjectionContext = createContext<MapProjection | null>(null);

/** For overlays rendered inside <MapView> (popovers etc.). */
export function useMapProjection(): MapProjection | null {
  return useContext(MapProjectionContext);
}

interface MapViewProps {
  markers: MapMarker[];
  routes: MapRoute[];
  bounds: LngLatBounds;
  /** One scalar, or four edges when chrome floats over the map. An object
      literal here re-fits on every render — memoise it at the call site. */
  padding?: number | EdgePadPx;
  onMarkerClick?: (markerId: string) => void;
  onMapClick?: () => void;
  className?: string;
  style?: CSSProperties;
  /** Overlays, absolutely positioned inside the map frame. */
  children?: ReactNode;
}

/** Declarative React face of the MapRenderer port. */
export function MapView({
  markers,
  routes,
  bounds,
  padding = 28,
  onMarkerClick,
  onMapClick,
  className,
  style,
  children,
}: MapViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [renderer] = useState<MapRenderer>(createMapRenderer);
  const [version, setVersion] = useState(0);

  useEffect(() => {
    renderer.mount(hostRef.current!);
    renderer.onViewChange(() => setVersion((v) => v + 1));
    return () => renderer.destroy();
  }, [renderer]);

  useEffect(() => {
    renderer.fitBounds(bounds, padding);
  }, [renderer, bounds, padding]);

  useEffect(() => {
    renderer.setRoutes(routes);
  }, [renderer, routes]);

  useEffect(() => {
    renderer.setMarkers(markers);
  }, [renderer, markers]);

  // Handlers may close over fresh state each render — always pass the latest.
  useEffect(() => {
    renderer.onMarkerClick(onMarkerClick ?? null);
    renderer.onMapClick(onMapClick ?? null);
  });

  const projection = useMemo<MapProjection>(
    () => ({ project: (p) => renderer.project(p), version }),
    [renderer, version],
  );

  return (
    <div className={`map-frame${className ? ` ${className}` : ''}`} style={style}>
      <div ref={hostRef} className="map-host" />
      <MapProjectionContext.Provider value={projection}>{children}</MapProjectionContext.Provider>
    </div>
  );
}
