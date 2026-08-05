import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from 'react-router';
import { useApi } from '../api/useApi';
import { invalidateTripPlanning } from '../api/queryInvalidation';
import { BackHome } from '../components/BackHome';
import { ChangeList } from './governanceShared';
import type { Edit, Place, PlanDetail, ReviewItem, User } from '../api/types';
import { useI18n } from '../i18n';

/**
 * The AI airlock (DESIGN.md §7): everything an API token drafted on my behalf,
 * waiting for my personal approve/dismiss. Approving a content edit applies it
 * (as me, "via AI"); approving a structural proposal merely publishes it — it
 * still faces leader approval or a poll. Nothing here touches the trip until I act.
 */
export function ReviewQueuePage() {
  const api = useApi();
  const { locale, t: ui } = useI18n();
  const queryClient = useQueryClient();
  const queue = useQuery({ queryKey: ['review-queue'], queryFn: () => api.getReviewQueue() });
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const tokens = useQuery({ queryKey: ['tokens'], queryFn: () => api.listTokens() });

  const items = queue.data ?? [];
  const decide = useMutation({
    mutationFn: ({ id, approve }: ReviewDecision) => (approve ? api.approveReviewItem(id) : api.rejectReviewItem(id)),
    onSuccess: async (_result, { approve, tripId, commentThreadId }) => {
      await invalidateTripPlanning(queryClient, tripId);
      if (approve && commentThreadId) {
        await queryClient.invalidateQueries({ queryKey: ['comments', tripId, commentThreadId] });
      }
    },
  });

  if (queue.isLoading) return <p className="muted">{ui('review.loading')}</p>;
  if (queue.isError) {
    return (
      <div className="rq-page">
        <BackHome />
        <div className="card rq-empty" role="alert">
          <strong>{ui('review.error.title')}</strong>
          <p className="muted">{ui('review.error.body')}</p>
          <button type="button" className="btn primary" onClick={() => queue.refetch()}>
            {ui('review.retry')}
          </button>
        </div>
      </div>
    );
  }

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
        <h1>{ui('review.title')}</h1>
        {items.length > 0 && <span className="count">{new Intl.NumberFormat(locale).format(items.length)}</span>}
        {token && (
          <span className={`token-chip${expired ? ' expired' : ''}`}>
            <span className="k" />
            {/* Each fact is its own no-wrap span, so a line break lands between
                two facts instead of splitting "itn_k7Jq…" down the middle. */}
            <span className="facts">
              <span>
                {ui('review.token.draftedBy')} <span className="mono">{token.name}</span>
              </span>
              <span className="mono">{token.prefix}…</span>
              <span>
                {ui('review.token.scopes')} {token.scopes.join(', ')}
              </span>
              <span className="until">
                {ui(expired ? 'review.token.expired' : 'review.token.expires')}{' '}
                {new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short' }).format(new Date(token.expiresAt))}
              </span>
            </span>
          </span>
        )}
      </div>
      <p className="muted rq-sub">{ui('review.subtitle')}</p>

      {/* Was a bare muted sentence floating under the sub-heading, which reads
          as a page that failed to load rather than one with nothing in it. */}
      {items.length === 0 && (
        <div className="card rq-empty">
          <span className="em" aria-hidden>
            ✓
          </span>
          <strong>{ui('review.empty.title')}</strong>
          <p className="muted">{ui('review.empty.body')}</p>
          <Link className="btn" to="/">
            {ui('review.empty.back')}
          </Link>
        </div>
      )}

      {items.map((item) => (
        <ReviewQueueItem
          key={item.id}
          item={item}
          meName={me.data?.displayName ?? ui('review.you')}
          deciding={decide.isPending && decide.variables?.id === item.id}
          decisionFailed={decide.isError && decide.variables?.id === item.id}
          onDecide={(approve) =>
            decide.mutate({
              id: item.id,
              approve,
              tripId: itemTripId(item),
              commentThreadId: item.kind === 'comment' ? item.comment.threadId : undefined,
            })
          }
        />
      ))}
    </div>
  );
}

interface ReviewDecision {
  id: string;
  approve: boolean;
  tripId: string;
  commentThreadId?: string;
}

function ReviewQueueItem({
  item,
  meName,
  deciding,
  decisionFailed,
  onDecide,
}: {
  item: ReviewItem;
  meName: string;
  deciding: boolean;
  decisionFailed: boolean;
  onDecide: (approve: boolean) => void;
}) {
  const api = useApi();
  const { t: ui } = useI18n();
  const tripId = itemTripId(item);
  const needsPlan = item.kind === 'proposal' || (item.kind === 'edit' && ['stop', 'day'].includes(item.edit.entity));
  const needsUsers = item.kind === 'comment';
  const needsCandidates = item.kind === 'proposal';
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const plan = useQuery({
    queryKey: ['plan', tripId],
    queryFn: () => api.getCurrentPlan(tripId!),
    enabled: !!tripId && needsPlan,
  });
  const users = useQuery({
    queryKey: ['users', tripId],
    queryFn: () => api.getUsers(tripId!),
    enabled: !!tripId && needsUsers,
  });
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId && needsCandidates,
  });

  const contextQueries = [
    trip,
    ...(needsPlan ? [plan] : []),
    ...(needsUsers ? [users] : []),
    ...(needsCandidates ? [candidates] : []),
  ];
  const contextLoading = !!tripId && contextQueries.some((query) => query.isLoading);
  const contextFailed = !tripId || contextQueries.some((query) => query.isError);
  const currentVersion = plan.data?.plan.version;
  const staleProposal =
    item.kind === 'proposal' &&
    currentVersion !== undefined &&
    item.proposal.changeSet.basePlanVersion !== currentVersion;
  const copy = reviewActionCopy(item.kind);
  const approvalDisabled = deciding || contextLoading || contextFailed || staleProposal;
  const dismissDisabled = deciding || contextLoading || contextFailed;

  return (
    <div className="card rq-item">
      {trip.data && <div className="rq-trip">{ui('review.trip', { trip: trip.data.name })}</div>}
      <ItemBody
        item={item}
        detail={plan.data ?? null}
        extraPlaces={(candidates.data ?? []).map((candidate) => candidate.place)}
        meName={meName}
        usersById={byId(users.data)}
      />
      {contextLoading && (
        <div className="rq-context muted" aria-live="polite">
          {ui('review.context.loading')}
        </div>
      )}
      {contextFailed && (
        <div className="rq-context error" role="alert">
          <span>{ui('review.context.error')}</span>
          <button
            type="button"
            className="btn small"
            onClick={() => contextQueries.forEach((query) => void query.refetch())}
          >
            {ui('review.retry')}
          </button>
        </div>
      )}
      {staleProposal && (
        <div className="rq-context warning" role="status">
          <strong>{ui('review.stale.title')}</strong>
          <span>
            {ui('review.stale.body', {
              base: item.kind === 'proposal' ? item.proposal.changeSet.basePlanVersion : '',
              current: currentVersion ?? '',
            })}
          </span>
        </div>
      )}
      {decisionFailed && (
        <div className="rq-context error" role="alert">
          {ui('review.decisionError')}
        </div>
      )}
      <div className="prop-actions">
        <button type="button" className="btn approve" disabled={approvalDisabled} onClick={() => onDecide(true)}>
          {deciding ? ui('review.working') : ui(copy.approve)}
        </button>
        <button type="button" className="btn danger" disabled={dismissDisabled} onClick={() => onDecide(false)}>
          {ui('review.dismiss')}
        </button>
        <span className="role-note">{ui(staleProposal ? 'review.stale.hint' : copy.hint)}</span>
      </div>
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
  const { t: ui } = useI18n();
  if (item.kind === 'proposal') {
    const token = item.proposal.source.via === 'token' ? item.proposal.source.tokenName : 'AI';
    return (
      <>
        <div className="rq-src">
          <span className="badge">{ui('review.badge.structuralProposal')}</span>
          <span className="via">{ui('review.suggestedByActsAs', { source: token, name: meName })}</span>
        </div>
        <div>
          <strong className="rq-title">{item.proposal.title}</strong>
          <p className="muted rq-rat">{item.proposal.rationale}</p>
        </div>
        {detail && <ChangeList ops={item.proposal.changeSet.ops} detail={detail} extraPlaces={extraPlaces} />}
        <div className="chg-impact">
          <span className="k">{ui('review.impact.note')}</span>
          <span className="body">{ui('review.impact.structural')}</span>
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
          <span className="badge">{ui('review.badge.contentEdit')}</span>
          <span className="via">
            {ui('review.targetWithSource', { target: editTargetLabel(item.edit, detail, ui), source: token })}
          </span>
        </div>
        <div className="diff">
          {isAppend ? (
            <>
              <span className="lbl">{ui('review.diff.adds')}</span>
              <span className="now">
                <mark>{now}</mark>
              </span>
            </>
          ) : (
            <>
              <span className="lbl">{ui('review.diff.was')}</span>
              <span className="was">{was}</span>
              <span className="lbl">{ui('review.diff.now')}</span>
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
        <span className="badge">{ui('review.badge.newIdea')}</span> <strong>{item.place.name}</strong> —{' '}
        {item.candidate.pitch}
      </div>
    );
  }
  const author = usersById.get(item.comment.author)?.displayName ?? ui('review.someone');
  return (
    <>
      <div className="rq-src">
        <span className="badge">{ui('review.badge.comment')}</span>{' '}
        {ui('review.commentOnBy', { thread: item.threadTitle, author })}
      </div>
      <p className="muted">{item.comment.body}</p>
    </>
  );
}

function editTargetLabel(edit: Edit, detail: PlanDetail | null, ui: ReturnType<typeof useI18n>['t']): string {
  const entity = ui(`review.entity.${edit.entity}`);
  const knownField = {
    booking: 'review.field.booking',
    notes: 'review.field.notes',
    plannedArrival: 'review.field.plannedArrival',
    body: 'review.field.body',
  } as const;
  const field =
    edit.field in knownField ? ui(knownField[edit.field as keyof typeof knownField]) : ui('review.field.other');
  if (edit.entity === 'stop' && detail) {
    const stop = detail.stops.find((s) => s.id === edit.entityId);
    const place = stop && detail.places.find((p) => p.id === stop.placeId);
    if (place) return `${entity} · ${place.name} · ${field}`;
  }
  return `${entity} · ${field}`;
}

function reviewActionCopy(kind: ReviewItem['kind']) {
  switch (kind) {
    case 'proposal':
      return { approve: 'review.approveProposal', hint: 'review.proposalHint' } as const;
    case 'edit':
      return { approve: 'review.approveEdit', hint: 'review.editHint' } as const;
    case 'candidate':
      return { approve: 'review.approveCandidate', hint: 'review.candidateHint' } as const;
    case 'comment':
      return { approve: 'review.approveComment', hint: 'review.commentHint' } as const;
  }
}

function itemTripId(item: ReviewItem): string {
  if (item.kind === 'edit') return item.edit.tripId;
  if (item.kind === 'proposal') return item.proposal.tripId;
  if (item.kind === 'candidate') return item.candidate.tripId;
  return item.tripId;
}

function byId(users: User[] | undefined): Map<string, User> {
  return new Map((users ?? []).map((u) => [u.id, u]));
}
