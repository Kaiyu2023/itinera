import { useMemo, useSyncExternalStore } from 'react';
import { skipToken, useQuery } from '@tanstack/react-query';
import { useApi } from '../api/useApi';
import type { User } from '../api/types';

const DESKTOP_QUERY = '(min-width: 1024px)';

function subscribeToDesktopQuery(onChange: () => void): () => void {
  const query = window.matchMedia(DESKTOP_QUERY);
  query.addEventListener('change', onChange);
  return () => query.removeEventListener('change', onChange);
}

function isDesktopViewport(): boolean {
  return window.matchMedia(DESKTOP_QUERY).matches;
}

/** True at the desktop breakpoint (≥1024px) — where the map gets a side panel. */
export function useIsDesktop(): boolean {
  return useSyncExternalStore(subscribeToDesktopQuery, isDesktopViewport, () => false);
}

/** Member profiles for a trip, as a lookup map — avatars & names everywhere. */
export function useMembers(tripId: string | undefined) {
  const api = useApi();
  const query = useQuery({
    queryKey: ['users', tripId],
    queryFn: tripId ? () => api.getUsers(tripId) : skipToken,
  });
  const byId = useMemo(() => new Map<string, User>((query.data ?? []).map((user) => [user.id, user])), [query.data]);
  return { ...query, byId };
}
