import { useEffect, useRef, useState } from 'react';
import { useApi } from '../api/useApi';
import type { Place } from '../api/types';

export interface StopSearchController {
  query: string;
  setQuery: (query: string) => void;
  results: Place[];
  loading: boolean;
  selectedId: string | null;
  select: (id: string | null) => void;
  selected: Place | null;
  /** Arm the next results batch to auto-select its first hit (deep links). */
  pickFirstOnNext: () => void;
  clear: () => void;
}

/** Debounced place search shared by candidate and add-stop composers. */
export function useStopSearch(tripId: string): StopSearchController {
  const api = useApi();
  const [query, setQuery] = useState('');
  const [resultSet, setResultSet] = useState<{ tripId: string; places: Place[] }>({ tripId, places: [] });
  const [loading, setLoading] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const pickFirstRef = useRef(false);
  const requestRef = useRef(0);

  // React renders once with new props before effects run. Tagging each result
  // batch with its trip means that render cannot expose the previous trip's
  // private place snapshots, even for a single frame.
  const results = resultSet.tripId === tripId ? resultSet.places : [];
  const visibleSelectedId = resultSet.tripId === tripId ? selectedId : null;

  useEffect(() => {
    requestRef.current += 1;
    pickFirstRef.current = false;
    setResultSet({ tripId, places: [] });
    setSelectedId(null);
    setLoading(false);
  }, [tripId]);

  useEffect(() => {
    const normalizedQuery = query.trim();
    if (!normalizedQuery) {
      setResultSet({ tripId, places: [] });
      setLoading(false);
      return;
    }

    setLoading(true);
    const requestId = ++requestRef.current;
    const timeout = window.setTimeout(() => {
      api
        .searchPlaces(tripId, normalizedQuery)
        .then((nextResults) => {
          if (requestRef.current !== requestId) return;
          setResultSet({ tripId, places: nextResults });
          setLoading(false);
          if (pickFirstRef.current) {
            pickFirstRef.current = false;
            setSelectedId(nextResults[0]?.id ?? null);
          }
        })
        .catch(() => {
          if (requestRef.current !== requestId) return;
          setResultSet({ tripId, places: [] });
          setLoading(false);
        });
    }, 250);

    return () => {
      window.clearTimeout(timeout);
      // Ignore a response that settles after the query changed or unmounted.
      requestRef.current += 1;
    };
  }, [api, query, tripId]);

  return {
    query,
    setQuery,
    results,
    loading,
    selectedId: visibleSelectedId,
    select: setSelectedId,
    selected: results.find((result) => result.id === visibleSelectedId) ?? null,
    pickFirstOnNext: () => {
      pickFirstRef.current = true;
    },
    clear: () => {
      requestRef.current += 1;
      pickFirstRef.current = false;
      setQuery('');
      setResultSet({ tripId, places: [] });
      setSelectedId(null);
      setLoading(false);
    },
  };
}
