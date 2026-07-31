import { useQuery } from '@tanstack/react-query';
import { Link, NavLink, Outlet } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { fillStyle } from '../lib/oklch';
import { ThemeToggle } from './ThemeToggle';

/** Top bar + content outlet. The shell stays put on every page — the Plan
    tab's map lives in a card below the trip hero, never over the chrome. */
export function AppShell() {
  const api = useApi();
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const queue = useQuery({ queryKey: ['review-queue'], queryFn: () => api.getReviewQueue() });

  return (
    /* `overflow-x: clip` here, on the one box that is exactly the viewport
       wide: full-bleed children (the trip ribbon) are 100vw, which overshoots
       whenever a classic scrollbar is present. It has to be an ancestor of the
       *whole* page rather than <main>, since clipping at main's box is the very
       thing full-bleed is escaping. `clip` and not `hidden` because hidden
       would make this a scroll container and break the sticky day scrubber. */
    <div style={{ minHeight: '100%', display: 'flex', flexDirection: 'column', overflowX: 'clip' }}>
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      {/* Glass, and sticky, which is the only reason it earns the material: the
          page scrolls underneath it, so there is something real to see through.
          The rules moved to index.css because a backdrop-filter needs a
          @supports fallback and a dark-theme tint, neither of which fits in a
          style prop. */}
      <header className="topbar">
        <Link
          to="/"
          style={{
            fontFamily: 'var(--font-display)',
            fontWeight: 700,
            fontSize: 'var(--text-lg)',
            color: 'var(--color-text)',
          }}
        >
          Itinera
        </Link>
        <a
          className="muted tagline credit"
          href="https://github.com/Kaiyu2023/itinera"
          target="_blank"
          rel="noreferrer"
        >
          By Kaiyu2023
        </a>
        <span style={{ flex: 1 }} />
        <ThemeToggle />
        <NavLink to="/review" className="queue-pill" style={{ textDecoration: 'none' }}>
          {/* The second word goes below 480px. Three controls, a wordmark and a
              byline do not fit on a 390px bar, and "Review queue" was the one
              that broke — onto two lines, which made the top bar taller than
              --topbar-height and pushed the pill out of its own pill. */}
          {/* One element, not a bare text node beside a span: the pill is a flex
              row with a 6px gap, which would otherwise open between the two
              words. */}
          <span>
            Review<i className="qp-word"> queue</i>
          </span>
          {queue.data && queue.data.length > 0 ? <span className="n">{queue.data.length}</span> : null}
        </NavLink>
        {me.data && (
          <span
            className="avatar"
            style={fillStyle(me.data.avatarColor)}
            title={me.data.displayName}
            role="img"
            aria-label={`Signed in as ${me.data.displayName}`}
          >
            {me.data.displayName[0]}
          </span>
        )}
      </header>
      <main
        id="main"
        tabIndex={-1}
        style={{ flex: 1, width: '100%', maxWidth: 960, margin: '0 auto', padding: 'var(--space-4)' }}
      >
        <Outlet />
      </main>
    </div>
  );
}
