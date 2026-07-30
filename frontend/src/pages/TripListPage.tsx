import { useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { formatDate, tripPhase } from '../components/hooks';
import { accentFrom } from '../lib/oklch';
import { useColorScheme } from '../theme/useTripTheme';
import { CreateTripForm } from './createTripForm';

export function TripListPage() {
  const api = useApi();
  const trips = useQuery({ queryKey: ['trips'], queryFn: () => api.listTrips() });
  const [params, setParams] = useSearchParams();
  const scheme = useColorScheme();

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
        <button className="btn accent" onClick={() => setCreating(true)}>
          + New trip
        </button>
      </div>
      <div className="trip-shelf">
        {trips.data?.map((t) => {
          const phase = tripPhase(t.startDate, t.endDate);
          // Same synthesis as the trip surfaces themselves — a card must not
          // theme itself from the raw hex while everything inside the trip
          // themes from the rebuilt one.
          const accent = accentFrom(t.accentColor, scheme);
          return (
            <Link
              key={t.id}
              to={`/trips/${t.id}`}
              className="trip-card accent-scope"
              style={accent ? ({ '--accent': accent } as CSSProperties) : undefined}
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
