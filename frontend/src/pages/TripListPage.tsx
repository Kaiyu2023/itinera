import { useQuery } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDate } from '../components/hooks';

export function TripListPage() {
  const api = useApi();
  const trips = useQuery({ queryKey: ['trips'], queryFn: () => api.listTrips() });

  if (trips.isLoading) return <p className="muted">Loading trips…</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
      <h1 style={{ fontSize: 'var(--text-xl)' }}>Your trips</h1>
      {trips.data?.map((t) => (
        <Link key={t.id} to={`/trips/${t.id}`} className="card" style={{ display: 'block', color: 'inherit' }}>
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--space-3)' }}>
            <h2 style={{ fontSize: 'var(--text-lg)', flex: 1 }}>{t.name}</h2>
            <span className="badge">{t.status}</span>
          </div>
          <p className="muted">
            {formatDate(t.startDate)} → {formatDate(t.endDate)} · {t.memberCount} travellers
          </p>
          <p className="muted">{t.cities.join(' · ')}</p>
        </Link>
      ))}
    </div>
  );
}
