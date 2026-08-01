import type { QueryClient } from '@tanstack/react-query';

const TRIP_QUERY_ROOTS = [
  'trip',
  'plan',
  'candidates',
  'polls',
  'proposals',
  'history',
  'threads',
  'notices',
  'ledger',
  'users',
] as const;

/** Refresh data a trip-planning mutation can affect without refetching unrelated app state. */
export async function invalidateTripPlanning(queryClient: QueryClient, tripId: string): Promise<void> {
  await Promise.all([
    ...TRIP_QUERY_ROOTS.map((root) => queryClient.invalidateQueries({ queryKey: [root, tripId] })),
    queryClient.invalidateQueries({ queryKey: ['review-queue'] }),
    queryClient.invalidateQueries({ queryKey: ['trips'] }),
  ]);
}
