import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import type { Poll } from '../api/types';

export function PollsTab() {
  const { tripId } = useParams();
  const api = useApi();
  const polls = useQuery({ queryKey: ['polls', tripId], queryFn: () => api.listPolls(tripId!), enabled: !!tripId });

  if (polls.isLoading) return <p className="muted">Loading polls…</p>;

  const open = (polls.data ?? []).filter((p) => p.status === 'open');
  const closed = (polls.data ?? []).filter((p) => p.status !== 'open');

  return (
    <div style={{ display: 'grid', gap: 'var(--space-5)' }}>
      <section style={{ display: 'grid', gap: 'var(--space-3)' }}>
        <h2 style={{ fontSize: 'var(--text-lg)' }}>Open</h2>
        {open.length === 0 && <p className="muted">Nothing to vote on right now.</p>}
        {open.map((poll) => (
          <PollCard key={poll.id} poll={poll} />
        ))}
      </section>
      <section style={{ display: 'grid', gap: 'var(--space-3)' }}>
        <h2 style={{ fontSize: 'var(--text-lg)' }}>Closed</h2>
        {closed.map((poll) => (
          <PollCard key={poll.id} poll={poll} />
        ))}
      </section>
    </div>
  );
}

function PollCard({ poll }: { poll: Poll }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const members = useMembers(poll.tripId);
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });

  const vote = useMutation({
    mutationFn: (optionId: string) => api.vote(poll.id, [optionId]),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['polls', poll.tripId] }),
  });

  const myVote = poll.votes.find((v) => v.userId === me.data?.id)?.optionId;
  const isOpen = poll.status === 'open';
  const counts = new Map<string, number>();
  for (const v of poll.votes) counts.set(v.optionId, (counts.get(v.optionId) ?? 0) + 1);
  const winner = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];

  return (
    <div className="card">
      <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
        <strong>{poll.title}</strong>
        {poll.kind === 'plan_change' && <span className="badge">plan change</span>}
        <span className={`badge ${poll.status === 'passed' ? 'ok' : ''}`}>{poll.status}</span>
        <span className="muted" style={{ flex: 1, textAlign: 'right' }}>
          {isOpen
            ? `closes ${new Date(poll.closesAt).toLocaleDateString()}`
            : `${poll.votes.length} votes · quorum ${poll.quorum}`}
        </span>
      </div>
      {poll.description && <p className="muted" style={{ marginTop: 'var(--space-1)' }}>{poll.description}</p>}

      <div style={{ display: 'grid', gap: 'var(--space-2)', marginTop: 'var(--space-3)' }}>
        {poll.options.map((option) => {
          const votersHere = poll.votes.filter((v) => v.optionId === option.id);
          const isMine = myVote === option.id;
          return (
            <button
              key={option.id}
              disabled={!isOpen || vote.isPending}
              onClick={() => vote.mutate(option.id)}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 'var(--space-2)',
                width: '100%',
                textAlign: 'left',
                padding: 'var(--space-2) var(--space-3)',
                borderRadius: 'var(--radius-sm)',
                border: `1px solid ${isMine ? 'var(--color-primary)' : 'var(--color-border)'}`,
                background: isMine ? 'color-mix(in srgb, var(--color-primary) 10%, transparent)' : 'var(--color-surface)',
                color: 'inherit',
                cursor: isOpen ? 'pointer' : 'default',
              }}
            >
              <span style={{ flex: 1 }}>
                {option.label}
                {!isOpen && option.id === winner && poll.status === 'passed' && (
                  <span className="badge ok" style={{ marginLeft: 'var(--space-2)' }}>winner</span>
                )}
              </span>
              {votersHere.map((v) => {
                const user = members.byId.get(v.userId);
                return user ? (
                  <span key={v.userId} className="avatar" style={{ background: user.avatarColor, width: 22, height: 22 }} title={user.displayName}>
                    {user.displayName[0]}
                  </span>
                ) : null;
              })}
            </button>
          );
        })}
      </div>
    </div>
  );
}
