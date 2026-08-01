import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import type { TripStatus } from '../api/types';
import { useI18n, type MessageKey } from '../i18n';

/**
 * Moving a trip along its lifecycle.
 *
 * The status pill in the hero already said `Planning`; it just wasn't a
 * control, so the five phases the type declares were something the fixtures
 * could express and the app could not. Making the pill itself the button is
 * the whole design — it is where you already look to find out what phase you
 * are in, so it is where you will look to change it.
 *
 * Drawn as a ladder rather than a dropdown, because the phases are ordered and
 * the order is the information. Every rung stays clickable, though: `booked` →
 * `planning` is a real thing that happens when a booking falls through, and the
 * moment it does is the moment you least want the app arguing with you.
 *
 * The payoff is visible immediately, which is the point of putting it here.
 * `--env-amplitude` is keyed to status (docs/VISUAL-DESIGN.md §6.2), so picking
 * a phase changes how loud the whole page is allowed to be — a trip you are
 * dreaming about looks like weather, an itinerary you are navigating from on a
 * train looks like a document.
 */

interface Phase {
  key: TripStatus;
  label: MessageKey;
  blurb: MessageKey;
}

const PHASES: Phase[] = [
  { key: 'dreaming', label: 'trip.status.dreaming', blurb: 'trip.status.dreamingBlurb' },
  { key: 'planning', label: 'trip.status.planning', blurb: 'trip.status.planningBlurb' },
  { key: 'booked', label: 'trip.status.booked', blurb: 'trip.status.bookedBlurb' },
  { key: 'ongoing', label: 'trip.status.ongoing', blurb: 'trip.status.ongoingBlurb' },
  { key: 'done', label: 'trip.status.done', blurb: 'trip.status.doneBlurb' },
];

export function TripStatusPicker({ tripId, status }: { tripId: string; status: TripStatus }) {
  const api = useApi();
  const { t } = useI18n();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [at, setAt] = useState({ top: 0, left: 0 });
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);

  const move = useMutation({
    mutationFn: (next: TripStatus) => api.setTripStatus(tripId, next),
    onSuccess: () => {
      // The list carries the same status, and the trip query drives the page's
      // whole environment — both have to move together or the hero and the sky
      // disagree until the next navigation.
      queryClient.invalidateQueries({ queryKey: ['trip', tripId] });
      queryClient.invalidateQueries({ queryKey: ['trips'] });
      setOpen(false);
      buttonRef.current?.focus();
    },
  });

  // The panel is portalled to <body> and positioned in viewport coordinates.
  // It has to be: the hero it hangs off is `overflow: hidden` so its cover
  // photo can have rounded corners, which sliced the panel off two rungs down.
  // Escaping that clip in place would mean unclipping the photo.
  useLayoutEffect(() => {
    if (!open) return;
    const r = buttonRef.current?.getBoundingClientRect();
    if (!r) return;
    const width = Math.min(290, window.innerWidth - 24);
    setAt({ top: r.bottom + 8, left: Math.max(12, Math.min(r.left, window.innerWidth - width - 12)) });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false);
        buttonRef.current?.focus();
      }
    };
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!wrapRef.current?.contains(t) && !menuRef.current?.contains(t)) setOpen(false);
    };
    // Fixed coordinates go stale the moment the page moves, and the hero this
    // hangs off scrolls away entirely. Closing is the honest response.
    const onScroll = () => setOpen(false);
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onDown);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', onScroll);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', onScroll);
    };
  }, [open]);

  const menuRef = useRef<HTMLDivElement | null>(null);
  const currentIndex = PHASES.findIndex((p) => p.key === status);
  const current = PHASES[currentIndex] ?? PHASES[0];

  return (
    <div className="status-wrap" ref={wrapRef}>
      <button
        ref={buttonRef}
        type="button"
        className="badge frosted status-pill"
        aria-haspopup="true"
        aria-expanded={open}
        /* The visible text is one word, which is a poor name for a control:
           "Planning" does not say that pressing it does anything. */
        aria-label={t('trip.status.controlLabel', { phase: t(current.label) })}
        onClick={() => setOpen((v) => !v)}
        title={t('trip.status.changeTitle')}
      >
        {t(current.label)}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.4} strokeLinecap="round" aria-hidden>
          <path d="M6 9.5l6 6 6-6" />
        </svg>
      </button>

      {open &&
        createPortal(
          <div
            className="status-menu"
            role="menu"
            aria-label={t('trip.status.menuLabel')}
            ref={menuRef}
            style={{ top: at.top, left: at.left }}
          >
            <p className="sm-head">{t('trip.status.menuHeading')}</p>
            <ol>
              {PHASES.map((p, i) => (
                <li key={p.key}>
                  <button
                    type="button"
                    role="menuitem"
                    className={`sm-step${p.key === status ? ' now' : ''}${i < currentIndex ? ' past' : ''}`}
                    onClick={() => (p.key === status ? setOpen(false) : move.mutate(p.key))}
                    disabled={move.isPending}
                  >
                    <span className="sm-dot" aria-hidden />
                    <span className="sm-text">
                      <b>{t(p.label)}</b>
                      <em>{t(p.blurb)}</em>
                    </span>
                    {p.key === status && <span className="sm-now">{t('trip.status.current')}</span>}
                  </button>
                </li>
              ))}
            </ol>
            <p className="sm-foot">{t('trip.status.backwardsHint')}</p>
          </div>,
          document.body,
        )}
    </div>
  );
}
