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

/** Where a trip sits relative to today, with a human label for the hero pill. */
export function tripPhase(
  startDate: string,
  endDate: string,
): { phase: 'before' | 'during' | 'after'; label: string; short: string } {
  const dayMs = 86_400_000;
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const start = new Date(startDate + 'T00:00:00').getTime();
  const end = new Date(endDate + 'T00:00:00').getTime();
  if (today < start) {
    const days = Math.round((start - today) / dayMs);
    return {
      phase: 'before',
      label: days === 1 ? 'tomorrow!' : `${days} days to go`,
      short: `${days}d`,
    };
  }
  if (today > end) return { phase: 'after', label: 'trip complete', short: '✓' };
  const dayN = Math.round((today - start) / dayMs) + 1;
  return {
    phase: 'during',
    label: `Day ${dayN} of ${Math.round((end - start) / dayMs) + 1}`,
    short: `Day ${dayN}`,
  };
}

export function formatDate(iso: string): string {
  return new Date(iso + (iso.length === 10 ? 'T00:00:00' : '')).toLocaleDateString(undefined, {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
  });
}
