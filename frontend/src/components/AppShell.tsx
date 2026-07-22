import { useQuery } from '@tanstack/react-query';
import { Link, NavLink, Outlet } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';

/** Top bar + content outlet. The shell stays put on every page — the Plan
    tab's map lives in a card below the trip hero, never over the chrome. */
export function AppShell() {
  const api = useApi();
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const queue = useQuery({ queryKey: ['review-queue'], queryFn: () => api.getReviewQueue() });

  return (
    <div style={{ minHeight: '100%', display: 'flex', flexDirection: 'column' }}>
      <header
        style={{
          height: 'var(--topbar-height)',
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-4)',
          padding: '0 var(--space-4)',
          background: 'var(--color-surface)',
          borderBottom: '1px solid var(--color-border)',
          position: 'sticky',
          top: 0,
          zIndex: 10,
        }}
      >
        <Link
          to="/"
          style={{ fontFamily: 'var(--font-display)', fontWeight: 700, fontSize: 'var(--text-lg)', color: 'var(--color-text)' }}
        >
          Itinera
        </Link>
        <span className="muted tagline">journeys, planned together</span>
        <span style={{ flex: 1 }} />
        <NavLink to="/review" className="queue-pill" style={{ textDecoration: 'none' }}>
          Review queue{queue.data && queue.data.length > 0 ? <span className="n">{queue.data.length}</span> : null}
        </NavLink>
        {me.data && (
          <span className="avatar" style={{ background: me.data.avatarColor }} title={me.data.displayName}>
            {me.data.displayName[0]}
          </span>
        )}
      </header>
      <main style={{ flex: 1, width: '100%', maxWidth: 960, margin: '0 auto', padding: 'var(--space-4)' }}>
        <Outlet />
      </main>
    </div>
  );
}
