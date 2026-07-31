import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { PlaceThumb } from '../components/PlaceThumb';
import { CandidateComposer } from './candidateComposer';
import type { CandidateStatus, CandidateWithPlace } from '../api/types';

const SECTIONS: { status: CandidateStatus; title: string; defaultOpen: boolean }[] = [
  { status: 'shortlisted', title: 'Competing for a slot', defaultOpen: true },
  { status: 'in_plan', title: 'In the plan', defaultOpen: true },
  { status: 'rejected', title: 'Voted off', defaultOpen: false },
];

/* ── deep link: ?cand=new(&q=&pick=first) opens the composer, one-shot + self-stripping ── */
type CandLink = { open: boolean; query: string | null; pickFirst: boolean };
function readCandDeepLink(params: URLSearchParams): CandLink {
  return { open: params.get('cand') === 'new', query: params.get('q'), pickFirst: params.get('pick') === 'first' };
}
function stripCandDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  ['cand', 'q', 'pick'].forEach((k) => next.delete(k));
  return next;
}

export function CandidatesTab() {
  const { tripId } = useParams();
  const api = useApi();
  const navigate = useNavigate();
  const members = useMembers(tripId);
  const [params, setParams] = useSearchParams();
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });
  // The current plan feeds the composer's "already in the trip" hinting.
  const plan = useQuery({ queryKey: ['plan', tripId], queryFn: () => api.getCurrentPlan(tripId!), enabled: !!tripId });

  const [composer, setComposer] = useState<{ query?: string | null; pickFirst?: boolean } | null>(null);
  // Which sections are expanded — first two open, "Voted off" collapsed.
  const [open, setOpen] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(SECTIONS.map((s) => [s.status, s.defaultOpen])),
  );
  // A pitch lands at the bottom of "Competing for a slot", which on a real
  // shortlist is well below the fold: the composer just vanished and nothing
  // visibly happened. Reveal the new card and flash it briefly.
  const [flashId, setFlashId] = useState<string | null>(null);
  useEffect(() => {
    if (!flashId) return;
    let raf = 0;
    let tries = 0;
    // The list is still refetching when the composer closes, so wait for the
    // card to exist rather than assuming it is already mounted.
    const reveal = () => {
      const el = document.querySelector(`[data-flash-id="${flashId}"]`);
      if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      else if (tries++ < 60) raf = requestAnimationFrame(reveal);
    };
    reveal();
    const done = setTimeout(() => setFlashId(null), 2600);
    return () => {
      cancelAnimationFrame(raf);
      clearTimeout(done);
    };
  }, [flashId]);
  const booted = useRef(false);
  if (!booted.current && candidates.data) {
    booted.current = true;
    const link = readCandDeepLink(params);
    if (link.open) {
      setComposer({ query: link.query, pickFirst: link.pickFirst });
      setParams(stripCandDeepLink(params), { replace: true });
    }
  }

  if (candidates.isLoading) return <p className="muted">Loading candidates…</p>;

  const all = candidates.data ?? [];

  return (
    <div style={{ display: 'grid', gap: 'var(--space-5)' }}>
      <div className="m4-tab-head">
        {/* <h2>, not <h1>: TripLayout's hero already gives the page its single
            <h1> (the trip name), and a second one per tab left the document
            with two competing top-level headings. */}
        <h2>Candidates</h2>
        <span className="spacer" />
        <button type="button" className="btn accent" onClick={() => setComposer({})}>
          ＋ Pitch an idea
        </button>
      </div>

      {/* A trip nobody has pitched to used to render the heading and then ~900px
          of nothing: every section returned null and there was no fallback after
          the map. The per-section null is still right for the *partial* case (an
          empty "Voted off" shouldn't draw an empty box), so the zero state is a
          whole-tab one, keyed on there being no candidates at all. */}
      {all.length === 0 ? (
        <div className="cand-zero">
          <strong>No one has pitched anything yet.</strong>
          <p className="muted">
            A candidate is a place someone wants the group to consider — a restaurant, a detour, a museum — with a
            sentence on why. Nothing here touches the itinerary: candidates sit on a shortlist until someone proposes
            one for a specific day, which is when the plan's usual approval kicks in.
          </p>
          <button type="button" className="btn accent" onClick={() => setComposer({})}>
            ＋ Pitch the first idea
          </button>
        </div>
      ) : (
        SECTIONS.map(({ status, title }) => {
          const group = all.filter((c) => c.status === status);
          if (group.length === 0) return null;
          const isOpen = open[status];
          const bodyId = `cand-sec-${status}`;
          return (
            <section key={status} className="cand-section">
              <button
                type="button"
                className="cand-section-head"
                aria-expanded={isOpen}
                aria-controls={bodyId}
                onClick={() => setOpen((s) => ({ ...s, [status]: !s[status] }))}
              >
                <span className={`chev${isOpen ? ' open' : ''}`} aria-hidden>
                  ▸
                </span>
                <h3>{title}</h3>
                <span className="count-badge">{group.length}</span>
              </button>
              {/* The collapsed body is only visually clipped (grid-template-rows
                  animates to 0fr), so its cards stayed in the a11y tree and its
                  buttons stayed tabbable — a keyboard user could focus controls
                  inside a section they had just folded shut. `inert` takes the
                  whole subtree out of focus order and out of the a11y tree while
                  keeping the height transition. */}
              <div className={`cand-section-body${isOpen ? ' open' : ''}`} id={bodyId} inert={!isOpen}>
                <div className="cand-section-inner">
                  {group.map((c) => (
                    <CandidateCard
                      key={c.id}
                      candidate={c}
                      proposerName={members.byId.get(c.proposedBy)?.displayName}
                      flash={flashId === c.id}
                      onPropose={() => navigate(`/trips/${tripId}/plan?gov=addStop&mode=candidates&candidate=${c.id}`)}
                      // Rejecting sends the card to "Voted off", which is folded
                      // shut by default — so without this the button reads as
                      // "delete". Open the destination and flash it there.
                      onMoved={(to) => {
                        setOpen((s) => ({ ...s, [to]: true }));
                        setFlashId(c.id);
                      }}
                    />
                  ))}
                </div>
              </div>
            </section>
          );
        })
      )}

      {composer && tripId && (
        <CandidateComposer
          tripId={tripId}
          detail={plan.data ?? null}
          initialQuery={composer.query}
          pickFirst={composer.pickFirst}
          onAdded={setFlashId}
          onClose={() => setComposer(null)}
        />
      )}
    </div>
  );
}

/* ═══════════════ one candidate ═══════════════ */

function CandidateCard({
  candidate: c,
  proposerName,
  flash,
  onPropose,
  onMoved,
}: {
  candidate: CandidateWithPlace;
  proposerName: string | undefined;
  flash: boolean;
  onPropose: () => void;
  onMoved: (to: CandidateStatus) => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const setStatus = useMutation({
    mutationFn: (status: CandidateStatus) => api.setCandidateStatus(c.id, status),
    onSuccess: (moved) => {
      queryClient.invalidateQueries({ queryKey: ['candidates', c.tripId] });
      onMoved(moved.status);
    },
  });
  const rejected = c.status === 'rejected';

  return (
    /* Rejected candidates used to carry a blanket `opacity: 0.6`, which took
       their already-muted secondary text down to 2.45:1 — unreadable, and for
       no reason: they are not disabled, they are just out of the running. The
       de-emphasis is structural now (sunken card, a badge that says so, and a
       desaturated thumbnail), and every glyph keeps its full contrast. */
    <div
      className={`card cand-card cand-enter${rejected ? ' rejected' : ''}${flash ? ' new-flash' : ''}`}
      data-flash-id={c.id}
    >
      <div>
        <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
          <strong>{c.place.name}</strong>
          <span className="muted">{c.place.city}</span>
          {c.place.rating != null && <span className="muted">★ {c.place.rating}</span>}
          {rejected && <span className="badge impossible">voted off</span>}
          {c.tags.map((tag) => (
            <span key={tag} className="badge">
              {tag}
            </span>
          ))}
        </div>
        <p style={{ marginTop: 'var(--space-1)' }}>{c.pitch}</p>
        {proposerName && (
          <p className="muted" style={{ marginTop: 'var(--space-1)' }}>
            — {proposerName}
          </p>
        )}
        {/* Until now the tab could only ever *read* a `rejected` candidate:
            there was no way to produce one, so the "Voted off" section could
            only show fixtures and a shortlist could only ever grow. */}
        {c.status === 'shortlisted' && (
          <div className="cand-actions">
            <button type="button" className="btn primary sm cand-propose" onClick={onPropose}>
              Propose for the plan →
            </button>
            <button
              type="button"
              className="btn sm"
              disabled={setStatus.isPending}
              onClick={() => setStatus.mutate('rejected')}
            >
              Not for this trip
            </button>
          </div>
        )}
        {rejected && (
          <div className="cand-actions">
            <button
              type="button"
              className="btn sm"
              disabled={setStatus.isPending}
              onClick={() => setStatus.mutate('shortlisted')}
            >
              Bring it back
            </button>
          </div>
        )}
      </div>
      <PlaceThumb photos={c.place.photoUrls} name={c.place.name} />
    </div>
  );
}
