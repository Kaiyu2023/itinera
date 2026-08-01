import type { RouteObject } from 'react-router';
import { Navigate } from 'react-router';
import { AppLoading, AppRouteError } from './components/AppRouteStates';
import { AppShell } from './components/AppShell';

export const routes: RouteObject[] = [
  {
    element: <AppShell />,
    ErrorBoundary: AppRouteError,
    HydrateFallback: AppLoading,
    children: [
      {
        path: '/',
        lazy: async () => ({ Component: (await import('./pages/TripListPage')).TripListPage }),
      },
      {
        path: '/review',
        lazy: async () => ({ Component: (await import('./pages/ReviewQueuePage')).ReviewQueuePage }),
      },
      {
        path: '/trips/:tripId',
        lazy: async () => ({ Component: (await import('./pages/TripLayout')).TripLayout }),
        children: [
          { index: true, element: <Navigate to="plan" replace /> },
          {
            path: 'plan',
            lazy: async () => ({ Component: (await import('./pages/PlanTab')).PlanTab }),
          },
          {
            path: 'candidates',
            lazy: async () => ({ Component: (await import('./pages/CandidatesTab')).CandidatesTab }),
          },
          {
            path: 'polls',
            lazy: async () => ({ Component: (await import('./pages/PollsTab')).PollsTab }),
          },
          {
            path: 'ledger',
            lazy: async () => ({ Component: (await import('./pages/LedgerTab')).LedgerTab }),
          },
          {
            path: 'prep',
            lazy: async () => ({ Component: (await import('./pages/NoticesTab')).NoticesTab }),
          },
        ],
      },
    ],
  },
];
