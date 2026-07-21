import { useQuery } from '@tanstack/react-query';
import { NavLink, Outlet, useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDate, useMembers } from '../components/hooks';

const TABS = [
  { to: 'plan', label: 'Plan' },
  { to: 'candidates', label: 'Candidates' },
  { to: 'polls', label: 'Polls' },
  { to: 'ledger', label: 'Ledger' },
  { to: 'prep', label: 'Before you go' },
];

export function TripLayout() {
  const { tripId } = useParams();
  const api = useApi();
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const members = useMembers(tripId);

  if (trip.isLoading) return <p className="muted">Loading trip…</p>;
  if (!trip.data) return <p className="muted">Trip not found.</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
      <div>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--space-3)', flexWrap: 'wrap' }}>
          <h1 style={{ fontSize: 'var(--text-xl)' }}>{trip.data.name}</h1>
          <span className="badge">{trip.data.status}</span>
          <span className="muted">
            {formatDate(trip.data.startDate)} → {formatDate(trip.data.endDate)}
          </span>
        </div>
        <div style={{ display: 'flex', gap: 'var(--space-1)', marginTop: 'var(--space-2)' }}>
          {trip.data.members.map((m) => {
            const user = members.byId.get(m.userId);
            if (!user) return null;
            return (
              <span
                key={m.userId}
                className="avatar"
                style={{ background: user.avatarColor }}
                title={`${user.displayName}${m.role === 'leader' ? ' · leader' : ''}`}
              >
                {user.displayName[0]}
              </span>
            );
          })}
        </div>
      </div>

      <nav style={{ display: 'flex', gap: 'var(--space-2)', borderBottom: '1px solid var(--color-border)', overflowX: 'auto' }}>
        {TABS.map((tab) => (
          <NavLink
            key={tab.to}
            to={tab.to}
            style={({ isActive }) => ({
              padding: 'var(--space-2) var(--space-3)',
              whiteSpace: 'nowrap',
              color: isActive ? 'var(--color-primary)' : 'var(--color-text-muted)',
              borderBottom: isActive ? '2px solid var(--color-primary)' : '2px solid transparent',
              fontWeight: isActive ? 600 : 400,
            })}
          >
            {tab.label}
          </NavLink>
        ))}
      </nav>

      <Outlet />
    </div>
  );
}
