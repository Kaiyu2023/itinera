import type { CSSProperties, ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { NavLink, Outlet, useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatDate, tripPhase, useMembers } from '../components/hooks';
import { personalOpenCount } from './noticesShared';

const TABS = [
  { to: 'plan', label: 'Plan', short: 'Plan' },
  { to: 'candidates', label: 'Candidates', short: 'Ideas' },
  { to: 'polls', label: 'Polls', short: 'Polls' },
  { to: 'ledger', label: 'Ledger', short: 'Ledger' },
  { to: 'prep', label: 'Before you go', short: 'Prep' },
];

export function TripLayout() {
  const { tripId } = useParams();
  const api = useApi();
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const members = useMembers(tripId);

  // Counts for the mobile bottom-nav bubbles (cache shared with each tab).
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });
  const polls = useQuery({ queryKey: ['polls', tripId], queryFn: () => api.listPolls(tripId!), enabled: !!tripId });
  const notices = useQuery({
    queryKey: ['notices', tripId],
    queryFn: () => api.listNotices(tripId!),
    enabled: !!tripId,
  });

  // Mobile: swap the hero for a slim bar once it scrolls out of view.
  const heroRef = useRef<HTMLElement | null>(null);
  const [heroGone, setHeroGone] = useState(false);
  useEffect(() => {
    const hero = heroRef.current;
    if (!hero) return;
    const observer = new IntersectionObserver(([entry]) => setHeroGone(!entry.isIntersecting), {
      rootMargin: '-56px 0px 0px 0px',
    });
    observer.observe(hero);
    return () => observer.disconnect();
  }, [trip.data?.id]);

  // Wash the whole viewport background with the trip's accent while we're inside
  // a trip. Set on <body> (not just the content column) so the full page picks
  // it up; cleaned on unmount so the neutral trip list returns.
  const accent = trip.data?.accentColor;
  useEffect(() => {
    if (!accent) return;
    document.body.style.setProperty('--accent', accent);
    document.body.classList.add('trip-tinted', 'accent-scope');
    return () => {
      document.body.style.removeProperty('--accent');
      document.body.classList.remove('trip-tinted', 'accent-scope');
    };
  }, [accent]);

  if (trip.isLoading) return <p className="muted">Loading trip…</p>;
  if (!trip.data) return <p className="muted">Trip not found.</p>;

  const t = trip.data;
  const phase = tripPhase(t.startDate, t.endDate);
  const counts: Record<string, number> = {
    candidates: candidates.data?.filter((c) => c.status === 'shortlisted').length ?? 0,
    polls: polls.data?.filter((p) => p.status === 'open').length ?? 0,
    // Your personal outstanding prep items across the active notices.
    prep:
      notices.data && me.data
        ? personalOpenCount(
            notices.data,
            me.data.id,
            t.members.map((m) => m.userId),
          )
        : 0,
  };

  return (
    <div
      className="trip-scope accent-scope"
      style={t.accentColor ? ({ '--accent': t.accentColor } as CSSProperties) : undefined}
    >
      <div className={`trip-topbar${heroGone ? ' visible' : ''}`}>
        <span className="name">{t.name}</span>
        <span className="d">{phase.short}</span>
      </div>

      <section className="trip-hero" ref={heroRef}>
        {t.coverPhotoUrl && <img className="cover" src={t.coverPhotoUrl} alt="" />}
        <div className="body">
          <span className="badge frosted">{t.status}</span>
          <h1>{t.name}</h1>
          <div className="on-photo-meta">
            {formatDate(t.startDate)} → {formatDate(t.endDate)} · {t.members.length} travellers
          </div>
          <div className="hero-row">
            <span className="pill-countdown">{phase.label}</span>
            <span className="avatar-stack" role="list" aria-label="Travellers">
              {t.members.map((m) => {
                const user = members.byId.get(m.userId);
                if (!user) return null;
                return (
                  <span
                    key={m.userId}
                    className="avatar"
                    style={{ background: user.avatarColor }}
                    title={`${user.displayName}${m.role === 'leader' ? ' · leader' : ''}`}
                    role="listitem"
                    aria-label={`${user.displayName}${m.role === 'leader' ? ', trip leader' : ''}`}
                  >
                    {user.displayName[0]}
                  </span>
                );
              })}
            </span>
          </div>
        </div>
      </section>

      <nav className="tabbar" aria-label="Trip sections">
        {TABS.map((tab) => (
          <NavLink key={tab.to} to={tab.to}>
            {tab.label}
          </NavLink>
        ))}
      </nav>

      <Outlet />

      <nav className="bottom-nav" aria-label="Trip sections">
        {TABS.map((tab) => (
          <NavLink key={tab.to} to={tab.to}>
            {ICONS[tab.to]}
            {tab.short}
            {counts[tab.to] > 0 && <span className="bub">{counts[tab.to]}</span>}
          </NavLink>
        ))}
      </nav>
    </div>
  );
}

/* Stroke icons for the bottom nav; `route` echoes the favicon's dashed path. */
const icon = (children: ReactNode) => (
  <svg
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth={2}
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden
  >
    {children}
  </svg>
);

const ICONS: Record<string, ReactNode> = {
  plan: icon(
    <>
      <path d="M5 17.5c0-4.5 4.5-3.5 5.5-7.5S16 6 16 6" strokeDasharray="0.1 3.4" />
      <circle cx="5" cy="18.5" r="2.2" fill="currentColor" stroke="none" />
      <circle cx="17.5" cy="5.5" r="2.2" fill="currentColor" stroke="none" />
    </>,
  ),
  candidates: icon(<path d="M12 3.6l2.5 5.1 5.6.8-4 4 1 5.6-5.1-2.7-5.1 2.7 1-5.6-4-4 5.6-.8z" />),
  polls: icon(
    <>
      <path d="M5 20v-8" />
      <path d="M12 20V5" />
      <path d="M19 20v-5" />
    </>,
  ),
  ledger: icon(
    <>
      <rect x="3" y="6.5" width="18" height="13" rx="3" />
      <path d="M3 11h18" />
      <path d="M15 15.5h3" />
    </>,
  ),
  prep: icon(
    <>
      <rect x="4" y="4" width="16" height="16" rx="4" />
      <path d="M9 12.5l2.2 2.2 4.3-4.7" />
    </>,
  ),
};
