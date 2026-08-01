import { useState } from 'react';
import type { CSSProperties } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router';
import { useApi } from '../api/useApi';
import type { TripStatus } from '../api/types';
import { getLocalizedTripPhase, useI18n, type MessageKey } from '../i18n';
import { accentFrom, oklchToHex } from '../lib/oklch';
import { useOneShotDeepLink } from '../lib/useOneShotDeepLink';
import { useColorScheme } from '../theme/useTripTheme';
import { CreateTripForm } from './createTripForm';

function readCreateTripDeepLink(params: URLSearchParams): true | null {
  return params.get('trip') === 'new' ? true : null;
}

function stripCreateTripDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  next.delete('trip');
  return next;
}

export function TripListPage() {
  const api = useApi();
  const { locale, t: ui, formatDate, formatNumber } = useI18n();
  const trips = useQuery({ queryKey: ['trips'], queryFn: () => api.listTrips() });
  const [params, setParams] = useSearchParams();
  const scheme = useColorScheme();

  // ?trip=new opens the create form once, then strips itself (Plan-tab pattern).
  const [creating, setCreating] = useState(false);
  useOneShotDeepLink({
    searchParams: params,
    setSearchParams: setParams,
    read: readCreateTripDeepLink,
    strip: stripCreateTripDeepLink,
    onMatch: () => setCreating(true),
  });

  if (trips.isLoading) return <p className="muted">{ui('trips.loading')}</p>;
  if (trips.isError) return <p className="muted">{ui('trips.error')}</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
      <div className="page-head">
        <h1>{ui('trips.title')}</h1>
        <button type="button" className="btn accent" onClick={() => setCreating(true)}>
          + {ui('trips.new')}
        </button>
      </div>
      <div className="trip-shelf">
        {trips.data?.length === 0 && <p className="muted">{ui('trips.empty')}</p>}
        {trips.data?.map((t) => {
          const phase = getLocalizedTripPhase(t.startDate, t.endDate, locale);
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
              <span className="badge frosted">{ui(STATUS_LABEL[t.status])}</span>
              <div className="body">
                <h2>{t.name}</h2>
                <div className="on-photo-meta">
                  <span className="dot" />
                  <span>
                    {formatDate(t.startDate)} → {formatDate(t.endDate)}
                    {phase.phase === 'before' && ` · ${ui('trips.startsIn', { countdown: phase.short })}`}
                  </span>
                </div>
                <span className="on-photo-meta sub">
                  {t.cities.length > 0
                    ? t.cities.join(' · ')
                    : ui('trips.noRoute', {
                        count: formatNumber(t.memberCount),
                        travellers: ui(t.memberCount === 1 ? 'trips.traveller' : 'trips.travellers'),
                      })}
                </span>
              </div>
              {/* Last in the DOM, not first where the <img> sits, even though
                  both are `position: absolute` and paint order is decided by
                  z-index. Blink computes `text-transform: capitalize` word
                  boundaries over the DOM text *before* out-of-flow boxes are
                  taken out: with the monogram first, the status badge's text
                  ran on from it as "SKdreaming", so the badge silently stopped
                  capitalizing and read "dreaming" while every photo card read
                  "Dreaming". Nothing follows it here, so nothing can inherit
                  the same problem. */}
              {!t.coverPhotoUrl && (
                <span className="cover cover-mono" style={{ background: seasonGradient(t.startDate) }} aria-hidden>
                  <span className="mono">{monogram(t.name)}</span>
                </span>
              )}
            </Link>
          );
        })}
      </div>

      {creating && <CreateTripForm onClose={() => setCreating(false)} />}
    </div>
  );
}

const STATUS_LABEL: Record<TripStatus, MessageKey> = {
  dreaming: 'trip.status.dreaming',
  planning: 'trip.status.planning',
  booked: 'trip.status.booked',
  ongoing: 'trip.status.ongoing',
  done: 'trip.status.done',
};

/* ── the cover a trip has before it has a photo ────────────────────────────── */

/**
 * A trip you just created got the `.trip-card` fallback gradient — the same
 * gradient every other coverless trip got, since a new trip has no
 * `accentColor` either. Put two or three of them on the shelf and they were
 * literally identical slabs; the only thing distinguishing the cards was the
 * text at the bottom, so the shelf stopped being scannable by sight.
 *
 * Two things a brand-new trip does know: its name and when it happens. So the
 * cover is a monogram over a seasonal gradient — not decoration, the two
 * fastest handles you have on "which trip is this".
 *
 * Hue angles only; lightness and chroma are rebuilt in OKLCH exactly as the
 * photo-derived accent is (src/lib/oklch.ts). White display text is printed
 * over this, so "spring green" and "winter blue" must land at the *same*
 * perceptual lightness — which hand-picked hexes never do.
 */
const SEASON_HUE = {
  winter: 258, // blue hour over snow
  spring: 146, // new leaves
  summer: 196, // sea light
  autumn: 52, // turning maples
} as const;

function seasonOf(startDate: string): keyof typeof SEASON_HUE {
  // ISO date, read as calendar month rather than parsed — no timezone can move
  // a trip's start into the previous season on the way to a Date object.
  const month = Number(startDate.slice(5, 7));
  if (month <= 2 || month === 12) return 'winter';
  if (month <= 5) return 'spring';
  if (month <= 8) return 'summer';
  return 'autumn';
}

function seasonGradient(startDate: string): string {
  const h = SEASON_HUE[seasonOf(startDate)];
  // Dark end deep enough that the card's own scrim isn't doing all the work of
  // keeping the white title legible; the second hue is rotated 26° so the
  // gradient has some life in it rather than being one colour dimmed.
  return `linear-gradient(145deg, ${oklchToHex(0.44, 0.09, h)}, ${oklchToHex(0.24, 0.06, (h + 26) % 360)})`;
}

/** Up to two initials, skipping the words nobody would use to name the trip. */
function monogram(name: string): string {
  const words = name
    .split(/[\s,–—-]+/)
    .map((w) => w.replace(/[^\p{L}\p{N}]/gu, ''))
    .filter((w) => w.length > 0 && !/^(the|a|an|in|of|on|to|and|at|by|for)$/i.test(w));
  const source = words.length > 0 ? words : [name.trim()];
  return source
    .slice(0, 2)
    .map((w) => [...w][0]?.toUpperCase() ?? '')
    .join('');
}
