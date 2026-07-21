import { useQuery } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import type { User } from '../api/types';

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

export function formatMoney(amount: number, currency: string): string {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency,
    maximumFractionDigits: currency === 'JPY' ? 0 : 2,
  }).format(amount);
}

export function formatDate(iso: string): string {
  return new Date(iso + (iso.length === 10 ? 'T00:00:00' : '')).toLocaleDateString(undefined, {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
  });
}
