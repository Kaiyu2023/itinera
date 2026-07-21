import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';

export function NoticesTab() {
  const { tripId } = useParams();
  const api = useApi();
  const queryClient = useQueryClient();
  const members = useMembers(tripId);
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const notices = useQuery({ queryKey: ['notices', tripId], queryFn: () => api.listNotices(tripId!), enabled: !!tripId });

  const toggle = useMutation({
    mutationFn: ({ noticeId, itemId }: { noticeId: string; itemId: string }) => api.toggleChecklistItem(noticeId, itemId),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['notices', tripId] }),
  });

  if (notices.isLoading) return <p className="muted">Loading notices…</p>;

  const sorted = [...(notices.data ?? [])].sort((a, b) => Number(b.pinned) - Number(a.pinned));

  return (
    <div style={{ display: 'grid', gap: 'var(--space-3)' }}>
      {sorted.map((notice) => (
        <section key={notice.id} className="card">
          <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
            {notice.pinned && <span title="pinned">📌</span>}
            <strong>{notice.title}</strong>
            <span className="badge">{notice.category}</span>
          </div>
          <div style={{ marginTop: 'var(--space-2)', whiteSpace: 'pre-wrap' }} className="muted">
            {notice.body}
          </div>
          {notice.sourceUrl && (
            <p style={{ marginTop: 'var(--space-1)' }}>
              <a href={notice.sourceUrl} target="_blank" rel="noreferrer" className="muted">
                source ↗
              </a>
            </p>
          )}
          {notice.checklistItems.length > 0 && (
            <div style={{ marginTop: 'var(--space-3)', display: 'grid', gap: 'var(--space-1)' }}>
              {notice.checklistItems.map((item) => (
                <label key={item.id} style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={!!me.data && item.doneBy.includes(me.data.id)}
                    onChange={() => toggle.mutate({ noticeId: notice.id, itemId: item.id })}
                  />
                  <span style={{ flex: 1 }}>{item.text}</span>
                  {item.doneBy.map((userId) => {
                    const user = members.byId.get(userId);
                    return user ? (
                      <span key={userId} className="avatar" style={{ background: user.avatarColor, width: 20, height: 20 }} title={user.displayName}>
                        {user.displayName[0]}
                      </span>
                    ) : null;
                  })}
                </label>
              ))}
            </div>
          )}
        </section>
      ))}
    </div>
  );
}
