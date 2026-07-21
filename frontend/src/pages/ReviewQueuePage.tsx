import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import type { ReviewItem } from '../api/types';

/**
 * The AI airlock (DESIGN.md §7): everything an API token drafted on my
 * behalf, waiting for my personal approve/reject. Approving a content edit
 * applies it; approving a structural proposal merely publishes it — it still
 * faces leader approval or a poll.
 */
export function ReviewQueuePage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const queue = useQuery({ queryKey: ['review-queue'], queryFn: () => api.getReviewQueue() });

  const decide = useMutation({
    mutationFn: ({ id, approve }: { id: string; approve: boolean }) =>
      approve ? api.approveReviewItem(id) : api.rejectReviewItem(id),
    onSuccess: () => queryClient.invalidateQueries(),
  });

  if (queue.isLoading) return <p className="muted">Loading review queue…</p>;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
      <div>
        <h1 style={{ fontSize: 'var(--text-xl)' }}>Review queue</h1>
        <p className="muted">Drafts from your AI tokens. Nothing here touches the trip until you approve it.</p>
      </div>

      {queue.data?.length === 0 && <p className="muted">Queue is empty — your AI has been quiet.</p>}

      {queue.data?.map((item) => (
        <div key={item.id} className="card">
          <ItemBody item={item} />
          <div style={{ display: 'flex', gap: 'var(--space-2)', marginTop: 'var(--space-3)' }}>
            <button
              onClick={() => decide.mutate({ id: item.id, approve: true })}
              disabled={decide.isPending}
              style={{ padding: 'var(--space-1) var(--space-4)', borderRadius: 'var(--radius-sm)', border: 'none', background: 'var(--color-ok)', color: '#fff', fontWeight: 600 }}
            >
              Approve
            </button>
            <button
              onClick={() => decide.mutate({ id: item.id, approve: false })}
              disabled={decide.isPending}
              style={{ padding: 'var(--space-1) var(--space-4)', borderRadius: 'var(--radius-sm)', border: '1px solid var(--color-border)', background: 'transparent', color: 'var(--color-text)' }}
            >
              Reject
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

function ItemBody({ item }: { item: ReviewItem }) {
  if (item.kind === 'edit') {
    const tokenName = item.edit.source.via === 'token' ? item.edit.source.tokenName : '?';
    return (
      <div>
        <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
          <span className="badge">content edit</span>
          <strong>
            {item.edit.entity} · {item.edit.field}
          </strong>
          <span className="muted">via AI token “{tokenName}”</span>
        </div>
        <p className="muted" style={{ marginTop: 'var(--space-2)', textDecoration: 'line-through' }}>
          {String(item.edit.oldValue)}
        </p>
        <p style={{ marginTop: 'var(--space-1)' }}>{String(item.edit.newValue)}</p>
      </div>
    );
  }
  if (item.kind === 'proposal') {
    const tokenName = item.proposal.source.via === 'token' ? item.proposal.source.tokenName : '?';
    return (
      <div>
        <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
          <span className="badge">structural proposal</span>
          <strong>{item.proposal.title}</strong>
          <span className="muted">via AI token “{tokenName}”</span>
        </div>
        <p className="muted" style={{ marginTop: 'var(--space-2)' }}>{item.proposal.rationale}</p>
        <p className="muted" style={{ marginTop: 'var(--space-1)', fontSize: 'var(--text-xs)' }}>
          {item.proposal.changeSet.ops.length} operation(s) against plan v{item.proposal.changeSet.basePlanVersion} — approving
          publishes it for leader approval or a poll.
        </p>
      </div>
    );
  }
  if (item.kind === 'candidate') {
    return (
      <div>
        <span className="badge">new candidate</span> <strong>{item.place.name}</strong>
        <p className="muted" style={{ marginTop: 'var(--space-1)' }}>{item.candidate.pitch}</p>
      </div>
    );
  }
  return (
    <div>
      <span className="badge">comment</span> on <strong>{item.threadTitle}</strong>
      <p className="muted" style={{ marginTop: 'var(--space-1)' }}>{item.comment.body}</p>
    </div>
  );
}
