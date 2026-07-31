import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { BackHome } from '../components/BackHome';
import { ChangeList } from './governanceShared';
import type { Edit, Place, PlanDetail, ReviewItem, User } from '../api/types';

/**
 * The AI airlock (DESIGN.md §7): everything an API token drafted on my behalf,
 * waiting for my personal approve/dismiss. Approving a content edit applies it
 * (as me, "via AI"); approving a structural proposal merely publishes it — it
 * still faces leader approval or a poll. Nothing here touches the trip until I act.
 */
export function ReviewQueuePage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const queue = useQuery({ queryKey: ['review-queue'], queryFn: () => api.getReviewQueue() });
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const tokens = useQuery({ queryKey: ['tokens'], queryFn: () => api.listTokens() });

  const items = queue.data ?? [];
  const tripId = items.map(itemTripId).find(Boolean);
  const plan = useQuery({ queryKey: ['plan', tripId], queryFn: () => api.getCurrentPlan(tripId!), enabled: !!tripId });
  const users = useQuery({ queryKey: ['users', tripId], queryFn: () => api.getUsers(tripId!), enabled: !!tripId });
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });

  const decide = useMutation({
    mutationFn: ({ id, approve }: { id: string; approve: boolean }) =>
      approve ? api.approveReviewItem(id) : api.rejectReviewItem(id),
    onSuccess: () => queryClient.invalidateQueries(),
  });

  if (queue.isLoading) return <p className="muted">Loading review queue…</p>;

  const token = tokens.data?.find((t) => t.name === 'claude') ?? tokens.data?.[0];
  // An expiry in the past was rendered in the same muted ink as everything else
  // in the chip — the queue presented a dead token as the live source of these
  // drafts. Compare against now and say so; the drafts are still yours to
  // action, but nothing new will arrive from this token.
  const expired = !!token && new Date(token.expiresAt).getTime() < Date.now();

  return (
    <div className="rq-page">
      <BackHome />
      <div className="rq-head">
        <h1>Your review queue</h1>
        {items.length > 0 && <span className="count">{items.length}</span>}
        {token && (
          <span className={`token-chip${expired ? ' expired' : ''}`}>
            <span className="k" />
            {/* Each fact is its own no-wrap span, so a line break lands between
                two facts instead of splitting "itn_k7Jq…" down the middle. */}
            <span className="facts">
              <span>
                drafted by <span className="mono">{token.name}</span>
              </span>
              <span className="mono">{token.prefix}…</span>
              <span>scopes {token.scopes.join(', ')}</span>
              <span className="until">
                {expired ? 'expired' : 'expires'}{' '}
                {new Date(token.expiresAt).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}
              </span>
            </span>
          </span>
        )}
      </div>
      <p className="muted rq-sub">Nothing here touches the trip until you approve it.</p>

      {/* Was a bare muted sentence floating under the sub-heading, which reads
          as a page that failed to load rather than one with nothing in it. */}
      {items.length === 0 && (
        <div className="card rq-empty">
          <span className="em" aria-hidden>
            ✓
          </span>
          <strong>Nothing waiting on you</strong>
          <p className="muted">
            Anything an API token drafts on your behalf — a stop's notes, a proposed re-order, a candidate — lands here
            first and stays out of the trip until you approve it.
          </p>
          <Link className="btn" to="/">
            Back to your trips
          </Link>
        </div>
      )}

      {items.map((item) => (
        <div key={item.id} className="card rq-item">
          <ItemBody
            item={item}
            detail={plan.data ?? null}
            extraPlaces={(candidates.data ?? []).map((c) => c.place)}
            meName={me.data?.displayName ?? 'you'}
            usersById={byId(users.data)}
          />
          <div className="prop-actions">
            <button
              className="btn approve"
              disabled={decide.isPending}
              onClick={() => decide.mutate({ id: item.id, approve: true })}
            >
              {item.kind === 'proposal'
                ? 'Approve — publishes for a leader or a poll'
                : `Approve — applies now as your edit (via AI)`}
            </button>
            <button
              className="btn danger"
              disabled={decide.isPending}
              onClick={() => decide.mutate({ id: item.id, approve: false })}
            >
              Dismiss
            </button>
            <span className="role-note">
              {item.kind === 'proposal'
                ? 'Two-stage: approving here only lets it enter the normal structural flow. It never edits the plan on its own.'
                : 'Content edits apply immediately and land in the field-level, revertible history.'}
            </span>
          </div>
        </div>
      ))}
    </div>
  );
}

function ItemBody({
  item,
  detail,
  extraPlaces,
  meName,
  usersById,
}: {
  item: ReviewItem;
  detail: PlanDetail | null;
  extraPlaces: Place[];
  meName: string;
  usersById: Map<string, User>;
}) {
  if (item.kind === 'proposal') {
    const token = item.proposal.source.via === 'token' ? item.proposal.source.tokenName : 'AI';
    return (
      <>
        <div className="rq-src">
          <span className="badge">structural proposal</span>
          <span className="via">
            suggested by <b>{token}</b> — will act as <b>{meName}</b>
          </span>
        </div>
        <div>
          <strong className="rq-title">{item.proposal.title}</strong>
          <p className="muted rq-rat">{item.proposal.rationale}</p>
        </div>
        {detail && <ChangeList ops={item.proposal.changeSet.ops} detail={detail} extraPlaces={extraPlaces} />}
        <div className="chg-impact">
          <span className="k">Note</span>
          <span className="body">Structural — feasibility must re-run before it can apply.</span>
        </div>
      </>
    );
  }
  if (item.kind === 'edit') {
    const token = item.edit.source.via === 'token' ? item.edit.source.tokenName : 'AI';
    const was = String(item.edit.oldValue ?? '');
    const now = String(item.edit.newValue ?? '');
    const isAppend = was.trim() === '';
    return (
      <>
        <div className="rq-src">
          <span className="badge">content edit</span>
          <span className="via">
            {editTargetLabel(item.edit, detail)} — suggested by <b>{token}</b>
          </span>
        </div>
        <div className="diff">
          {isAppend ? (
            <>
              <span className="lbl">Adds</span>
              <span className="now">
                <mark>{now}</mark>
              </span>
            </>
          ) : (
            <>
              <span className="lbl">Was</span>
              <span className="was">{was}</span>
              <span className="lbl">Now</span>
              <span className="now">{now}</span>
            </>
          )}
        </div>
      </>
    );
  }
  if (item.kind === 'candidate') {
    return (
      <div className="rq-src">
        <span className="badge">new candidate</span> <strong>{item.place.name}</strong> — {item.candidate.pitch}
      </div>
    );
  }
  const author = usersById.get(item.comment.author)?.displayName ?? 'someone';
  return (
    <>
      <div className="rq-src">
        <span className="badge">comment</span> on <b>{item.threadTitle}</b> — by <b>{author}</b>
      </div>
      <p className="muted">{item.comment.body}</p>
    </>
  );
}

function editTargetLabel(edit: Edit, detail: PlanDetail | null): string {
  if (edit.entity === 'stop' && detail) {
    const stop = detail.stops.find((s) => s.id === edit.entityId);
    const place = stop && detail.places.find((p) => p.id === stop.placeId);
    if (place) return `stop · ${place.name} · ${edit.field}`;
  }
  return `${edit.entity} · ${edit.field}`;
}

function itemTripId(item: ReviewItem): string | undefined {
  if (item.kind === 'edit') return item.edit.tripId;
  if (item.kind === 'proposal') return item.proposal.tripId;
  if (item.kind === 'candidate') return item.candidate.tripId;
  return undefined;
}

function byId(users: User[] | undefined): Map<string, User> {
  return new Map((users ?? []).map((u) => [u.id, u]));
}
