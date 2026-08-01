import { createContext, useContext } from 'react';
import type { LngLat } from './MapRenderer';

export interface MapProjection {
  project: (position: LngLat) => { x: number; y: number } | null;
  /** Bumped on zoom, pan, and resize so overlays re-project. */
  version: number;
}

export const MapProjectionContext = createContext<MapProjection | null>(null);

/** Access the projection exposed by the nearest MapView overlay. */
export function useMapProjection(): MapProjection | null {
  return useContext(MapProjectionContext);
}
