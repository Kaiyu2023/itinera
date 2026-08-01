import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/useApi';
import { useMembers } from '../components/hooks';
import { KindGlyph } from '../components/KindGlyph';
import { PlaceGuide } from '../components/PlaceGuide';
import { PlaceThumb } from '../components/PlaceThumb';
import { SheetModal } from '../components/SheetModal';
import { CandidateComposer } from './candidateComposer';
import { GovModalHost } from './PlanGovernance';
import type { CandidateStatus, CandidateWithPlace } from '../api/types';
import { useI18n } from '../i18n';
import { useOneShotDeepLink } from '../lib/useOneShotDeepLink';
import { PLACE_KIND_STOP_KIND } from './planShared';

const SECTIONS: { status: CandidateStatus; defaultOpen: boolean }[] = [
  { status: 'shortlisted', defaultOpen: true },
  { status: 'in_plan', defaultOpen: true },
  { status: 'rejected', defaultOpen: false },
];

const SECTION_MESSAGE = {
  shortlisted: 'ideas.section.shortlisted',
  in_plan: 'ideas.section.inPlan',
  rejected: 'ideas.section.rejected',
} as const;

/* ── deep link: ?cand=new(&q=&pick=first) opens the composer, one-shot + self-stripping ── */
type CandLink = { query: string | null; pickFirst: boolean };
function readCandDeepLink(params: URLSearchParams): CandLink | null {
  if (params.get('cand') !== 'new') return null;
  return { query: params.get('q'), pickFirst: params.get('pick') === 'first' };
}
function stripCandDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  ['cand', 'q', 'pick'].forEach((k) => next.delete(k));
  return next;
}

export function CandidatesTab() {
  const { tripId } = useParams();
  const api = useApi();
  const { locale, t: ui } = useI18n();
  const queryClient = useQueryClient();
  const members = useMembers(tripId);
  const [params, setParams] = useSearchParams();
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  // The current plan feeds the composer's "already in the trip" hinting.
  const plan = useQuery({ queryKey: ['plan', tripId], queryFn: () => api.getCurrentPlan(tripId!), enabled: !!tripId });

  const [composer, setComposer] = useState<
    { kind: 'add'; query?: string | null; pickFirst?: boolean } | { kind: 'edit'; candidate: CandidateWithPlace } | null
  >(null);
  // Which sections are expanded — first two open, "Voted off" collapsed.
  const [open, setOpen] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(SECTIONS.map((s) => [s.status, s.defaultOpen])),
  );
  // A pitch lands at the bottom of "Competing for a slot", which on a real
  // shortlist is well below the fold: the composer just vanished and nothing
  // visibly happened. Reveal the new card and flash it briefly.
  const [flashId, setFlashId] = useState<string | null>(null);
  const [proposalCandidateId, setProposalCandidateId] = useState<string | null>(null);
  const [rejecting, setRejecting] = useState<CandidateWithPlace | null>(null);
  const prepareFirstPlan = useMutation({
    mutationFn: (candidate: CandidateWithPlace) => api.initializePlan(tripId!, { anchorPlaceId: candidate.placeId }),
    onSuccess: (detail, candidate) => {
      // Set the detail synchronously so the modal can mount from this click;
      // invalidation alone would leave the CTA apparently unresponsive during
      // the refetch (and the old 404 can otherwise win the race).
      queryClient.setQueryData(['plan', tripId], detail);
      queryClient.invalidateQueries({ queryKey: ['trip', tripId] });
      queryClient.invalidateQueries({ queryKey: ['trips'] });
      setProposalCandidateId(candidate.id);
    },
  });
  const setStatus = useMutation({
    mutationFn: ({ id, status }: { id: string; status: CandidateStatus }) => api.setCandidateStatus(id, status),
    onSuccess: (moved) => {
      queryClient.invalidateQueries({ queryKey: ['candidates', moved.tripId] });
      setOpen((current) => ({ ...current, [moved.status]: true }));
      setFlashId(moved.id);
      setRejecting(null);
    },
  });
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
  useOneShotDeepLink({
    ready: !!candidates.data,
    searchParams: params,
    setSearchParams: setParams,
    read: readCandDeepLink,
    strip: stripCandDeepLink,
    onMatch: (link) => setComposer({ kind: 'add', query: link.query, pickFirst: link.pickFirst }),
  });

  if (candidates.isLoading) return <p className="muted">{ui('ideas.loading')}</p>;

  const all = candidates.data ?? [];
  const days = [...(plan.data?.days ?? [])].sort((a, b) => a.date.localeCompare(b.date));
  const proposeCandidate = (candidate: CandidateWithPlace) => {
    if (days[0] && plan.data) setProposalCandidateId(candidate.id);
    else prepareFirstPlan.mutate(candidate);
  };

  return (
    <div style={{ display: 'grid', gap: 'var(--space-5)' }}>
      <div className="m4-tab-head">
        {/* <h2>, not <h1>: TripLayout's hero already gives the page its single
            <h1> (the trip name), and a second one per tab left the document
            with two competing top-level headings. */}
        <h2>{ui('ideas.title')}</h2>
        <span className="spacer" />
        <button type="button" className="btn accent" onClick={() => setComposer({ kind: 'add' })}>
          {ui('ideas.add')}
        </button>
      </div>

      {prepareFirstPlan.isError && (
        <p className="cand-plan-error" role="alert">
          {ui('ideas.planSetup.error')}
        </p>
      )}

      {/* A trip nobody has pitched to used to render the heading and then ~900px
          of nothing: every section returned null and there was no fallback after
          the map. The per-section null is still right for the *partial* case (an
          empty "Voted off" shouldn't draw an empty box), so the zero state is a
          whole-tab one, keyed on there being no candidates at all. */}
      {all.length === 0 ? (
        <div className="cand-zero">
          <strong>{ui('ideas.empty.title')}</strong>
          <p className="muted">{ui('ideas.empty.body')}</p>
          <button type="button" className="btn accent" onClick={() => setComposer({ kind: 'add' })}>
            {ui('ideas.empty.addFirst')}
          </button>
        </div>
      ) : (
        SECTIONS.map(({ status }) => {
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
                <h3>{ui(SECTION_MESSAGE[status])}</h3>
                <span className="count-badge">{new Intl.NumberFormat(locale).format(group.length)}</span>
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
                      busy={setStatus.isPending && setStatus.variables?.id === c.id}
                      proposeBusy={prepareFirstPlan.isPending}
                      onPropose={() => proposeCandidate(c)}
                      onReject={() => setRejecting(c)}
                      onRestore={() => setStatus.mutate({ id: c.id, status: 'shortlisted' })}
                      onEdit={() => setComposer({ kind: 'edit', candidate: c })}
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
          candidate={composer.kind === 'edit' ? composer.candidate : undefined}
          initialQuery={composer.kind === 'add' ? composer.query : undefined}
          pickFirst={composer.kind === 'add' ? composer.pickFirst : undefined}
          onSaved={setFlashId}
          onClose={() => setComposer(null)}
        />
      )}

      {proposalCandidateId && tripId && plan.data && days[0] && (
        <GovModalHost
          action={{
            kind: 'addStop',
            day: days[0],
            initialCandidateId: proposalCandidateId,
            allowDaySelection: true,
          }}
          close={() => setProposalCandidateId(null)}
          tripId={tripId}
          detail={plan.data}
          days={days}
          candidates={all}
          membersById={members.byId}
          threads={[]}
          isLeader={!!trip.data?.members.some((member) => member.userId === me.data?.id && member.role === 'leader')}
        />
      )}

      {rejecting && (
        <CandidateRejectDialog
          candidate={rejecting}
          busy={setStatus.isPending}
          onClose={() => setRejecting(null)}
          onConfirm={() => setStatus.mutate({ id: rejecting.id, status: 'rejected' })}
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
  busy,
  proposeBusy,
  onPropose,
  onReject,
  onRestore,
  onEdit,
}: {
  candidate: CandidateWithPlace;
  proposerName: string | undefined;
  flash: boolean;
  busy: boolean;
  proposeBusy: boolean;
  onPropose: () => void;
  onReject: () => void;
  onRestore: () => void;
  onEdit: () => void;
}) {
  const { locale, t: ui } = useI18n();
  const rejected = c.status === 'rejected';
  const kindMessage = {
    sight: 'ideas.kind.sight',
    food: 'ideas.kind.food',
    lodging: 'ideas.kind.lodging',
    activity: 'ideas.kind.activity',
    transport_hub: 'ideas.kind.transportHub',
  } as const;

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
      <div className="cand-main">
        <div className="cand-kind">
          <KindGlyph kind={PLACE_KIND_STOP_KIND[c.place.kind]} />
          {ui(kindMessage[c.place.kind])}
          {rejected && <span className="badge impossible">{ui('ideas.status.passedOn')}</span>}
        </div>
        <h4 className="cand-title">{c.place.name}</h4>
        <p className="cand-meta">
          {c.place.city}
          {c.place.rating != null && <> · ★ {new Intl.NumberFormat(locale).format(c.place.rating)}</>}
        </p>
        {!!c.tags.length && (
          <div className="cand-tags" aria-label={ui('ideas.tagsAria')}>
            {c.tags.map((tag) => (
              <span key={tag} className="badge">
                {tag}
              </span>
            ))}
          </div>
        )}
        <PlaceGuide
          place={c.place}
          tripContext={<p>{c.pitch}</p>}
          contextLabel={proposerName ? ui('ideas.whyPerson', { name: proposerName }) : ui('ideas.whyGeneric')}
          variant="disclosure"
          headingLevel="h5"
        />
        {/* Until now the tab could only ever *read* a `rejected` candidate:
            there was no way to produce one, so the "Voted off" section could
            only show fixtures and a shortlist could only ever grow. */}
        {c.status === 'shortlisted' && (
          <div className="cand-actions">
            <button type="button" className="btn primary sm cand-propose" onClick={onPropose} disabled={proposeBusy}>
              {proposeBusy ? ui('ideas.planSetup.preparing') : ui('ideas.proposeForDay')}
            </button>
            <button type="button" className="btn sm" onClick={onEdit}>
              {ui('ideas.edit')}
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={onReject}>
              {ui('ideas.pass')}
            </button>
          </div>
        )}
        {rejected && (
          <div className="cand-actions">
            <button type="button" className="btn sm" onClick={onEdit}>
              {ui('ideas.edit')}
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={onRestore}>
              {ui('ideas.reconsider')}
            </button>
          </div>
        )}
      </div>
      <PlaceThumb photos={c.place.photoUrls} name={c.place.name} />
    </div>
  );
}

function CandidateRejectDialog({
  candidate,
  busy,
  onClose,
  onConfirm,
}: {
  candidate: CandidateWithPlace;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { t: ui } = useI18n();
  return (
    <SheetModal onClose={onClose}>
      <div
        className="exp-modal cand-reject-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cand-reject-title"
        aria-describedby="cand-reject-copy"
        aria-busy={busy}
      >
        <span className="cand-reject-grip" aria-hidden />
        <header className="cand-reject-head">
          <span className="cand-reject-mark" aria-hidden>
            <svg viewBox="0 0 24 24">
              <path d="M5 7.5h14M8 7.5v10h8v-10M9.5 4.5h5M10 11h4" />
            </svg>
          </span>
          <span className="cand-reject-title">
            <span>{ui('ideas.reject.copyStatus')}</span>
            <h2 id="cand-reject-title">{ui('ideas.reject.title', { place: candidate.place.name })}</h2>
          </span>
          <button type="button" className="cand-reject-close" onClick={onClose} aria-label={ui('common.close')}>
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d="m6 6 12 12M18 6 6 18" />
            </svg>
          </button>
        </header>
        <div className="cand-reject-body" id="cand-reject-copy">
          <p>{ui('ideas.reject.impact', { status: ui('ideas.reject.copyStatus') })}</p>
          <p className="cand-reject-reversible">
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d="M8 8H4v-4M4.5 8a8 8 0 1 1-.3 7" />
            </svg>
            <span>{ui('ideas.reject.reversible')}</span>
          </p>
        </div>
        <footer className="cand-reject-actions">
          <button type="button" className="btn cand-reject-cancel" onClick={onClose} disabled={busy}>
            {ui('ideas.reject.keep')}
          </button>
          <button type="button" className="btn danger cand-reject-confirm" onClick={onConfirm} disabled={busy}>
            {ui('ideas.reject.confirm')}
          </button>
        </footer>
      </div>
    </SheetModal>
  );
}
