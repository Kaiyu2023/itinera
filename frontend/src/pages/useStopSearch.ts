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
export function useStopSearch(): StopSearchController {
  const api = useApi();
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<Place[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const pickFirstRef = useRef(false);
  const requestRef = useRef(0);

  useEffect(() => {
    const normalizedQuery = query.trim();
    if (!normalizedQuery) {
      setResults([]);
      setLoading(false);
      return;
    }

    setLoading(true);
    const requestId = ++requestRef.current;
    const timeout = window.setTimeout(() => {
      api
        .searchPlaces(normalizedQuery)
        .then((nextResults) => {
          if (requestRef.current !== requestId) return;
          setResults(nextResults);
          setLoading(false);
          if (pickFirstRef.current) {
            pickFirstRef.current = false;
            setSelectedId(nextResults[0]?.id ?? null);
          }
        })
        .catch(() => {
          if (requestRef.current !== requestId) return;
          setResults([]);
          setLoading(false);
        });
    }, 250);

    return () => {
      window.clearTimeout(timeout);
      // Ignore a response that settles after the query changed or unmounted.
      requestRef.current += 1;
    };
  }, [api, query]);

  return {
    query,
    setQuery,
    results,
    loading,
    selectedId,
    select: setSelectedId,
    selected: results.find((result) => result.id === selectedId) ?? null,
    pickFirstOnNext: () => {
      pickFirstRef.current = true;
    },
    clear: () => {
      requestRef.current += 1;
      pickFirstRef.current = false;
      setQuery('');
      setResults([]);
      setSelectedId(null);
      setLoading(false);
    },
  };
}
