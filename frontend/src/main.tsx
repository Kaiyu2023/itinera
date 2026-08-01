import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createBrowserRouter, RouterProvider } from 'react-router';
import { ApiProvider } from './api/ApiProvider';
import { routes } from './App';
import { AppErrorState, AppLoading } from './components/AppRouteStates';
import './index.css';

// PWA: production builds only — dev must keep instant HMR with no SW cache
// in the way. sw.js caches the shell + static assets so an already-visited
// plan still opens with no roaming data.
if ('serviceWorker' in navigator && import.meta.env.PROD) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => {
      /* offline support is progressive — never block the app on it */
    });
  });
}

const rootElement = document.getElementById('root');
if (!rootElement) throw new Error('The application root element is missing.');

const root = createRoot(rootElement);
root.render(<AppLoading />);

async function startApp(): Promise<void> {
  // Phase A runs against in-memory fixtures. Loading that large catalog as its
  // own chunk keeps the application shell small; Phase B swaps this import for
  // the HTTP client without changing the component tree.
  const { MockApiClient } = await import('./api/mock/MockApiClient');
  const client = new MockApiClient();
  const queryClient = new QueryClient({
    defaultOptions: { queries: { staleTime: 30_000, retry: 1 } },
  });

  root.render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <ApiProvider client={client}>
          <RouterProvider router={createBrowserRouter(routes)} />
        </ApiProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
}

void startApp().catch((error: unknown) => {
  console.error('Itinera could not start.', error);
  root.render(<AppErrorState onRetry={() => window.location.reload()} />);
});
