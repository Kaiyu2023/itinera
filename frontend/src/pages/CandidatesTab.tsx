import { useQuery } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import type { CandidateStatus } from '../api/types';

const SECTIONS: { status: CandidateStatus; title: string }[] = [
  { status: 'shortlisted', title: 'Competing for a slot' },
  { status: 'in_plan', title: 'In the plan' },
  { status: 'rejected', title: 'Voted off' },
];

export function CandidatesTab() {
  const { tripId } = useParams();
  const api = useApi();
  const members = useMembers(tripId);
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });

  if (candidates.isLoading) return <p className="muted">Loading candidates…</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-5)' }}>
      {SECTIONS.map(({ status, title }) => {
        const group = (candidates.data ?? []).filter((c) => c.status === status);
        if (group.length === 0) return null;
        return (
          <section key={status}>
            <h2 style={{ fontSize: 'var(--text-lg)', marginBottom: 'var(--space-3)' }}>{title}</h2>
            <div style={{ display: 'grid', gap: 'var(--space-3)' }}>
              {group.map((c) => {
                const proposer = members.byId.get(c.proposedBy);
                return (
                  <div key={c.id} className="card" style={{ opacity: status === 'rejected' ? 0.6 : 1 }}>
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
                  </div>
                );
              })}
            </div>
          </section>
        );
      })}
    </div>
  );
}
