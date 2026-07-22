import { useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDate, tripPhase } from '../components/hooks';
import { CreateTripForm } from './createTripForm';

export function TripListPage() {
  const api = useApi();
  const trips = useQuery({ queryKey: ['trips'], queryFn: () => api.listTrips() });
  const [params, setParams] = useSearchParams();

  // ?trip=new opens the create form once, then strips itself (Plan-tab pattern).
  const [creating, setCreating] = useState(false);
  const booted = useRef(false);
  if (!booted.current) {
    booted.current = true;
    if (params.get('trip') === 'new') {
      setCreating(true);
      const next = new URLSearchParams(params);
      next.delete('trip');
      setParams(next, { replace: true });
    }
  }

  if (trips.isLoading) return <p className="muted">Loading trips…</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
      <div className="page-head">
        <h1>Your trips</h1>
        <button className="btn accent" onClick={() => setCreating(true)}>+ New trip</button>
      </div>
      <div className="trip-shelf">
        {trips.data?.map((t) => {
          const phase = tripPhase(t.startDate, t.endDate);
          return (
            <Link
              key={t.id}
              to={`/trips/${t.id}`}
              className="trip-card"
              style={t.accentColor ? ({ '--trip-accent': t.accentColor } as CSSProperties) : undefined}
            >
              {t.coverPhotoUrl && <img className="cover" src={t.coverPhotoUrl} alt="" />}
              <span className="badge frosted">{t.status}</span>
              <div className="body">
                <h2>{t.name}</h2>
                <div className="on-photo-meta">
                  <span className="dot" />
                  <span>
                    {formatDate(t.startDate)} → {formatDate(t.endDate)}
                    {phase.phase === 'before' && ` · in ${phase.short}`}
                  </span>
                </div>
                <span className="on-photo-meta sub">
                  {t.cities.length > 0
                    ? t.cities.join(' · ')
                    : `${t.memberCount} ${t.memberCount === 1 ? 'traveller' : 'travellers'} · no route yet`}
                </span>
              </div>
            </Link>
          );
        })}
      </div>

      {creating && <CreateTripForm onClose={() => setCreating(false)} />}
    </div>
  );
}
