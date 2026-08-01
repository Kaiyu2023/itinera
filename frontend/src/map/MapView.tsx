import { useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { useI18n } from '../i18n';
import type { EdgePadPx, LngLatBounds, MapMarker, MapRenderer, MapRoute, MapUiLabels } from './MapRenderer';
import { MockMapRenderer } from './MockMapRenderer';
import { MapProjectionContext } from './MapProjectionContext';
import type { MapProjection } from './MapProjectionContext';

/**
 * Renderer selection lives here and nowhere else. When GoogleMapRenderer
 * exists (Phase B), an env var / trip setting picks it — no caller changes.
 */
function createMapRenderer(labels: MapUiLabels): MapRenderer {
  return new MockMapRenderer(labels);
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
  const { t } = useI18n();
  const hostRef = useRef<HTMLDivElement>(null);
  const labels = useMemo<MapUiLabels>(
    () => ({
      zoomIn: t('common.map.zoomIn'),
      zoomOut: t('common.map.zoomOut'),
      attribution: t('common.map.attribution'),
    }),
    [t],
  );
  const [renderer] = useState<MapRenderer>(() => createMapRenderer(labels));
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
    renderer.setUiLabels(labels);
  }, [renderer, labels]);

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
