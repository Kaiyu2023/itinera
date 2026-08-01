import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import type { User } from '../api/types';

/** True at the desktop breakpoint (≥1024px) — where the map gets a side panel. */
export function useIsDesktop(): boolean {
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1024px)').matches);
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1024px)');
    const onChange = () => setIsDesktop(mq.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);
  return isDesktop;
}

/** Member profiles for a trip, as a lookup map — avatars & names everywhere. */
export function useMembers(tripId: string | undefined) {
  const api = useApi();
  const query = useQuery({
    queryKey: ['users', tripId],
    queryFn: () => api.getUsers(tripId!),
    enabled: !!tripId,
  });
  const byId = new Map<string, User>((query.data ?? []).map((u) => [u.id, u]));
  return { ...query, byId };
}
