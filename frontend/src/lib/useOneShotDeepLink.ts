import { useEffect, useEffectEvent, useRef } from 'react';
import type { SetURLSearchParams } from 'react-router';

interface OneShotDeepLinkOptions<T> {
  /** Wait for any data needed to interpret the link before consuming it. */
  ready?: boolean;
  searchParams: URLSearchParams;
  setSearchParams: SetURLSearchParams;
  read: (params: URLSearchParams) => T | null;
  strip: (params: URLSearchParams) => URLSearchParams;
  onMatch: (match: T) => void;
}

/**
 * Consume a page-opening deep link at most once during this mount.
 *
 * State updates and URL replacement happen from an effect, keeping render
 * pure while preserving the existing one-shot behavior.
 */
export function useOneShotDeepLink<T>({
  ready = true,
  searchParams,
  setSearchParams,
  read,
  strip,
  onMatch,
}: OneShotDeepLinkOptions<T>): void {
  const bootstrapped = useRef(false);
  const consume = useEffectEvent((params: URLSearchParams) => {
    const match = read(params);
    if (match === null) return;

    onMatch(match);
    setSearchParams(strip(params), { replace: true });
  });

  useEffect(() => {
    if (!ready || bootstrapped.current) return;
    bootstrapped.current = true;
    consume(searchParams);
  }, [ready, searchParams]);
}
