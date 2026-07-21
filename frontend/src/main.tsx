import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createBrowserRouter, RouterProvider } from 'react-router-dom';
import { ApiProvider } from './api/ApiProvider';
import { MockApiClient } from './api/mock/MockApiClient';
import { routes } from './App';
import './index.css';

// Phase A: the entire app runs against in-memory fixtures.
// Phase B cutover = replace this one line with HttpApiClient.
const client = new MockApiClient();

const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 30_000, retry: 1 } },
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ApiProvider client={client}>
        <RouterProvider router={createBrowserRouter(routes)} />
      </ApiProvider>
    </QueryClientProvider>
  </StrictMode>,
);
