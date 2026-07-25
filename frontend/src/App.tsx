import type { RouteObject } from 'react-router';
import { Navigate } from 'react-router';
import { AppShell } from './components/AppShell';
import { TripListPage } from './pages/TripListPage';
import { TripLayout } from './pages/TripLayout';
import { PlanTab } from './pages/PlanTab';
import { CandidatesTab } from './pages/CandidatesTab';
import { PollsTab } from './pages/PollsTab';
import { LedgerTab } from './pages/LedgerTab';
import { NoticesTab } from './pages/NoticesTab';
import { ReviewQueuePage } from './pages/ReviewQueuePage';

export const routes: RouteObject[] = [
  {
    element: <AppShell />,
    children: [
      { path: '/', element: <TripListPage /> },
      { path: '/review', element: <ReviewQueuePage /> },
      {
        path: '/trips/:tripId',
        element: <TripLayout />,
        children: [
          { index: true, element: <Navigate to="plan" replace /> },
          { path: 'plan', element: <PlanTab /> },
          { path: 'candidates', element: <CandidatesTab /> },
          { path: 'polls', element: <PollsTab /> },
          { path: 'ledger', element: <LedgerTab /> },
          { path: 'prep', element: <NoticesTab /> },
        ],
      },
    ],
  },
];
