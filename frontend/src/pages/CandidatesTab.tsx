import { useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { PlaceThumb } from '../components/PlaceThumb';
import { CandidateComposer } from './candidateComposer';
import type { CandidateStatus } from '../api/types';

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
  const [open, setOpen] = useState<Record<string, boolean>>(
    () => Object.fromEntries(SECTIONS.map((s) => [s.status, s.defaultOpen])),
  );
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

  return (
    <div style={{ display: 'grid', gap: 'var(--space-5)' }}>
      <div className="m4-tab-head">
        <h1>Candidates</h1>
        <span className="spacer" />
        <button type="button" className="btn accent" onClick={() => setComposer({})}>＋ Pitch an idea</button>
      </div>

      {SECTIONS.map(({ status, title }) => {
        const group = (candidates.data ?? []).filter((c) => c.status === status);
        if (group.length === 0) return null;
        const isOpen = open[status];
        return (
          <section key={status} className="cand-section">
            <button
              type="button"
              className="cand-section-head"
              aria-expanded={isOpen}
              onClick={() => setOpen((s) => ({ ...s, [status]: !s[status] }))}
            >
              <span className={`chev${isOpen ? ' open' : ''}`} aria-hidden>▸</span>
              <h2>{title}</h2>
              <span className="count-badge">{group.length}</span>
            </button>
            <div className={`cand-section-body${isOpen ? ' open' : ''}`}>
              <div className="cand-section-inner">
                {group.map((c) => {
                  const proposer = members.byId.get(c.proposedBy);
                  return (
                    <div key={c.id} className="card cand-card cand-enter" style={{ opacity: status === 'rejected' ? 0.6 : 1 }}>
                      <div>
                        <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
                          <strong>{c.place.name}</strong>
                          <span className="muted">{c.place.city}</span>
                          {c.place.rating != null && <span className="muted">★ {c.place.rating}</span>}
                          {c.tags.map((tag) => (
                            <span key={tag} className="badge">{tag}</span>
                          ))}
                        </div>
                        <p style={{ marginTop: 'var(--space-1)' }}>{c.pitch}</p>
                        {proposer && (
                          <p className="muted" style={{ marginTop: 'var(--space-1)' }}>
                            — {proposer.displayName}
                          </p>
                        )}
                        {status === 'shortlisted' && (
                          <div className="cand-actions">
                            <button
                              type="button"
                              className="btn primary sm cand-propose"
                              onClick={() => navigate(`/trips/${tripId}/plan?gov=addStop&mode=candidates&candidate=${c.id}`)}
                            >
                              Propose for the plan →
                            </button>
                          </div>
                        )}
                      </div>
                      <PlaceThumb photos={c.place.photoUrls} name={c.place.name} />
                    </div>
                  );
                })}
              </div>
            </div>
          </section>
        );
      })}

      {composer && tripId && (
        <CandidateComposer
          tripId={tripId}
          detail={plan.data ?? null}
          initialQuery={composer.query}
          pickFirst={composer.pickFirst}
          onClose={() => setComposer(null)}
        />
      )}
    </div>
  );
}
