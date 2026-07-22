import { useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { ChangeList } from './governanceShared';
import { PollComposer } from './pollComposer';
import type { Place, Poll, PlanDetail, Proposal } from '../api/types';

/**
 * Governance home (DESIGN.md §4.2): proposals awaiting a decision, open polls
 * with live voting + quorum, the other lifecycle states, and the decided
 * history. Leaders get approve / reject / route-to-poll; members see status.
 */
export function PollsTab() {
  const { tripId } = useParams();
  const api = useApi();
  const polls = useQuery({ queryKey: ['polls', tripId], queryFn: () => api.listPolls(tripId!), enabled: !!tripId });
  const proposals = useQuery({
    queryKey: ['proposals', tripId],
    queryFn: () => api.listProposals(tripId!),
    enabled: !!tripId,
  });
  const plan = useQuery({ queryKey: ['plan', tripId], queryFn: () => api.getCurrentPlan(tripId!), enabled: !!tripId });
  const trip = useQuery({ queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId });
  const candidates = useQuery({
    queryKey: ['candidates', tripId],
    queryFn: () => api.listCandidates(tripId!),
    enabled: !!tripId,
  });
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });

  // ?poll=new opens the composer once, then strips itself (Plan-tab pattern).
  const [params, setParams] = useSearchParams();
  const [composing, setComposing] = useState(false);
  const booted = useRef(false);
  if (!booted.current && polls.data) {
    booted.current = true;
    if (params.get('poll') === 'new') {
      setComposing(true);
      const next = new URLSearchParams(params);
      next.delete('poll');
      setParams(next, { replace: true });
    }
  }

  if (polls.isLoading || proposals.isLoading) return <p className="muted">Loading governance…</p>;

  const isLeader = !!trip.data?.members.some((m) => m.userId === me.data?.id && m.role === 'leader');
  const detail = plan.data ?? null;
  const extraPlaces = (candidates.data ?? []).map((c) => c.place);

  // A pending proposal wrapped by a live poll is the group's to decide — it
  // shows as that poll, not in the leader queue (no racing decision paths).
  // If the poll dies (closed below quorum), it falls back here.
  const pollWrapped = new Set(
    (polls.data ?? [])
      .filter((pl) => pl.status === 'open' || pl.status === 'scheduled' || pl.status === 'draft')
      .flatMap((pl) => pl.options.map((o) => o.proposalId).filter((id): id is string => !!id)),
  );
  const pending = (proposals.data ?? []).filter((p) => p.status === 'pending' && !pollWrapped.has(p.id));
  const history = (proposals.data ?? []).filter((p) => p.status === 'applied' || p.status === 'rejected');
  const open = (polls.data ?? []).filter((p) => p.status === 'open');
  const upcoming = (polls.data ?? []).filter((p) => p.status === 'draft' || p.status === 'scheduled');
  const decided = (polls.data ?? []).filter(
    (p) => p.status === 'passed' || p.status === 'failed' || p.status === 'expired',
  );

  return (
    <div className="gov-tab">
      <div className="m4-tab-head">
        <h1>Governance</h1>
        <span className="spacer" />
        <button type="button" className="btn accent" onClick={() => setComposing(true)}>
          ＋ New poll
        </button>
      </div>

      {pending.length > 0 && (
        <section className="gov-sec">
          <h2 className="gov-h">Awaiting a decision</h2>
          {pending.map((p) => (
            <ProposalCard key={p.id} proposal={p} detail={detail} extraPlaces={extraPlaces} isLeader={isLeader} />
          ))}
        </section>
      )}

      <section className="gov-sec">
        <h2 className="gov-h">Open polls</h2>
        {open.length === 0 && <p className="muted">Nothing to vote on right now.</p>}
        {open.map((poll) => (
          <PollCard
            key={poll.id}
            poll={poll}
            detail={detail}
            proposals={proposals.data ?? []}
            extraPlaces={extraPlaces}
            isLeader={isLeader}
            meId={me.data?.id}
          />
        ))}
      </section>

      {upcoming.length > 0 && (
        <section className="gov-sec">
          <h2 className="gov-h">Drafts &amp; scheduled</h2>
          {upcoming.map((poll) => (
            <PollCard
              key={poll.id}
              poll={poll}
              detail={detail}
              proposals={proposals.data ?? []}
              extraPlaces={extraPlaces}
              isLeader={isLeader}
              meId={me.data?.id}
            />
          ))}
        </section>
      )}

      {(decided.length > 0 || history.length > 0) && (
        <section className="gov-sec">
          <h2 className="gov-h">Decided</h2>
          {decided.map((poll) => (
            <PollCard
              key={poll.id}
              poll={poll}
              detail={detail}
              proposals={proposals.data ?? []}
              extraPlaces={extraPlaces}
              isLeader={isLeader}
              meId={me.data?.id}
            />
          ))}
          {history.map((p) => (
            <ProposalCard key={p.id} proposal={p} detail={detail} extraPlaces={extraPlaces} isLeader={isLeader} />
          ))}
        </section>
      )}

      {composing && tripId && <PollComposer tripId={tripId} onClose={() => setComposing(false)} />}
    </div>
  );
}

/* ═══════════════ quorum meter ═══════════════ */

function QuorumMeter({ poll, memberCount }: { poll: Poll; memberCount: number }) {
  const voted = new Set(poll.votes.map((v) => v.userId)).size;
  const met = voted >= poll.quorum;
  const pips = Array.from({ length: memberCount }, (_, i) => i);
  const decided = poll.status !== 'open';
  return (
    <div className="quorum">
      <span className="pips">
        {pips.map((i) => (
          <span key={i} className={`pip${i < voted ? ' on' : ''}${i === poll.quorum - 1 ? ' q' : ''}`} />
        ))}
      </span>
      <span>
        <b>
          {voted} of {memberCount}
        </b>{' '}
        {decided ? '' : 'voted '}· quorum {poll.quorum}{' '}
        {met ? <span className="met">✓ {decided ? '' : 'met'}</span> : <span className="nomet">✗ not met</span>}
      </span>
    </div>
  );
}

/* ═══════════════ poll card ═══════════════ */

function PollCard({
  poll,
  detail,
  proposals,
  extraPlaces,
  isLeader,
  meId,
}: {
  poll: Poll;
  detail: PlanDetail | null;
  proposals: Proposal[];
  extraPlaces: Place[];
  isLeader: boolean;
  meId: string | undefined;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const members = useMembers(poll.tripId);
  const refresh = () => queryClient.invalidateQueries();

  const vote = useMutation({ mutationFn: (optionId: string) => api.vote(poll.id, [optionId]), onSuccess: refresh });
  const openMut = useMutation({ mutationFn: () => api.openPoll(poll.id), onSuccess: refresh });
  const closeMut = useMutation({ mutationFn: () => api.closePoll(poll.id), onSuccess: refresh });

  const myVote = poll.votes.find((v) => v.userId === meId)?.optionId;
  const isOpen = poll.status === 'open';
  const counts = new Map<string, number>();
  for (const v of poll.votes) counts.set(v.optionId, (counts.get(v.optionId) ?? 0) + 1);
  const maxCount = Math.max(1, ...counts.values());
  const winnerId = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];

  const changeProposal =
    poll.kind === 'plan_change'
      ? proposals.find((p) => p.id === poll.options.find((o) => o.proposalId)?.proposalId)
      : undefined;

  const daysLeft = Math.max(0, Math.ceil((new Date(poll.closesAt).getTime() - Date.now()) / 86_400_000));

  return (
    <div className={`card poll poll-${poll.status}`}>
      <div className="poll-top">
        <strong>{poll.title}</strong>
        <span className={`badge${poll.kind === 'plan_change' ? ' plan' : ''}`}>
          {poll.kind === 'plan_change' ? 'plan change' : 'decision'}
        </span>
        <span className={`badge ${statusBadge(poll.status)}`}>{poll.status}</span>
        <span className="meta">
          {poll.status === 'scheduled' && poll.opensAt ? (
            `opens ${dayShort(poll.opensAt)}`
          ) : isOpen ? (
            <>
              closes {dayTime(poll.closesAt)}
              <br />
              {daysLeft} {daysLeft === 1 ? 'day' : 'days'} left
            </>
          ) : poll.status === 'expired' ? (
            `closed ${dayShort(poll.closesAt)}`
          ) : poll.status === 'passed' || poll.status === 'failed' ? (
            `decided ${dayShort(poll.closesAt)}`
          ) : null}
        </span>
      </div>
      {poll.description && <p className="poll-desc">{poll.description}</p>}
      <p className="poll-by">
        Opened by <b>{members.byId.get(poll.createdBy)?.displayName ?? '—'}</b>
        {changeProposal && (
          <>
            {' '}
            · wraps <b>{changeProposal.id}</b> against Plan v{changeProposal.changeSet.basePlanVersion}
          </>
        )}
      </p>

      {changeProposal && detail && (
        <div className="preview">
          <span className="block-h">
            What adopting changes — Plan v{changeProposal.changeSet.basePlanVersion} → v
            {changeProposal.changeSet.basePlanVersion + 1}
          </span>
          <ChangeList ops={changeProposal.changeSet.ops} detail={detail} extraPlaces={extraPlaces} />
        </div>
      )}

      <div className="poll-opts">
        {poll.options.map((option) => {
          const votersHere = poll.votes.filter((v) => v.optionId === option.id);
          const isMine = myVote === option.id;
          const isWin = !isOpen && option.id === winnerId && poll.status === 'passed';
          const canVote = isOpen;
          return (
            <button
              key={option.id}
              type="button"
              className={`opt${isMine ? ' mine' : ''}${isWin ? ' win' : ''}`}
              disabled={!canVote || vote.isPending}
              onClick={() => canVote && vote.mutate(option.id)}
            >
              <span className="fill" style={{ width: `${Math.max(8, (votersHere.length / maxCount) * 100)}%` }} />
              <span className="lab">
                {option.label}
                {isMine && <span className="yours">· your vote</span>}
                {isWin && <span className="badge ok">winner</span>}
              </span>
              <span className="voters">
                {isMine && <span className="tick">✓</span>}
                <span className="stack">
                  {votersHere.map((v) => {
                    const u = members.byId.get(v.userId);
                    return u ? (
                      <span
                        key={v.userId}
                        className="avatar sm"
                        style={{ background: u.avatarColor }}
                        title={u.displayName}
                      >
                        {u.displayName[0]}
                      </span>
                    ) : null;
                  })}
                </span>
                <span className="cnt">{votersHere.length}</span>
              </span>
            </button>
          );
        })}
      </div>

      {poll.status !== 'draft' && poll.status !== 'scheduled' && (
        <QuorumMeter poll={poll} memberCount={members.byId.size || 6} />
      )}

      {poll.resolutionNote && (
        <p className="hint" style={{ marginTop: 2 }}>
          {poll.resolutionNote}
        </p>
      )}

      <div className="poll-foot">
        {poll.status === 'draft' && (
          <>
            <span className="hint">Draft — no votes counted yet.</span>
            <span className="spacer" />
            <button className="btn solid sm" disabled={!isLeader || openMut.isPending} onClick={() => openMut.mutate()}>
              Open poll
            </button>
          </>
        )}
        {poll.status === 'scheduled' && (
          <>
            <span className="hint">Auto-opens on schedule.</span>
            <span className="spacer" />
            <button className="btn sm" disabled={!isLeader || openMut.isPending} onClick={() => openMut.mutate()}>
              Open now
            </button>
          </>
        )}
        {isOpen && (
          <>
            {myVote ? (
              <span className="prompt">
                You voted <b>{poll.options.find((o) => o.id === myVote)?.label.split(' (')[0]}</b>. Tap another option
                to change it.
              </span>
            ) : (
              <>
                <span className="prompt">You haven't voted —</span>
                <span className="hint">tap an option to cast. Changeable until close.</span>
              </>
            )}
            <span className="spacer" />
            {poll.kind === 'plan_change' && (
              <span className="hint">
                Passes → applies as <b>Plan v{(changeProposal?.changeSet.basePlanVersion ?? 3) + 1}</b>.
              </span>
            )}
            {isLeader && (
              <button className="btn ghost-link sm" disabled={closeMut.isPending} onClick={() => closeMut.mutate()}>
                Close now
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** "Fri 24 Jul" — the app's compact date, matching the mockup's poll meta. */
function dayShort(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
}

/** "Sat 25 Jul, 13:00" — date + local time, for an open poll's close moment. */
function dayTime(iso: string): string {
  return `${dayShort(iso)}, ${new Date(iso).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
}

function statusBadge(status: Poll['status']): string {
  if (status === 'passed') return 'ok';
  if (status === 'expired' || status === 'failed') return 'unreasonable';
  if (status === 'open') return 'open';
  return '';
}

/* ═══════════════ proposal card ═══════════════ */

function ProposalCard({
  proposal,
  detail,
  extraPlaces,
  isLeader,
}: {
  proposal: Proposal;
  detail: PlanDetail | null;
  extraPlaces: Place[];
  isLeader: boolean;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const members = useMembers(proposal.tripId);
  const refresh = () => queryClient.invalidateQueries();
  const [rejecting, setRejecting] = useState(false);
  const [reason, setReason] = useState('');

  const approve = useMutation({ mutationFn: () => api.approveProposal(proposal.id), onSuccess: refresh });
  const reject = useMutation({
    mutationFn: () => api.rejectProposal(proposal.id, reason),
    onSuccess: () => {
      setRejecting(false);
      refresh();
    },
  });
  const toPoll = useMutation({ mutationFn: () => api.proposalToPoll(proposal.id), onSuccess: refresh });

  const author = members.byId.get(proposal.createdBy)?.displayName ?? '—';
  const nextVersion = proposal.changeSet.basePlanVersion + 1;
  const decider =
    proposal.decidedBy?.kind === 'leader' ? members.byId.get(proposal.decidedBy.userId)?.displayName : undefined;
  const isPending = proposal.status === 'pending';
  const createdLabel = new Date(proposal.createdAt).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });

  return (
    <div className="card prop">
      <div className="prop-head">
        <span className="avatar" style={{ background: members.byId.get(proposal.createdBy)?.avatarColor ?? '#888' }}>
          {author[0]}
        </span>
        <div className="ti">
          <strong>{proposal.title}</strong>
          <div className="tags">
            <span className="badge">structural</span>
            {isPending && isLeader && <span className="badge tight">needs a leader or a poll</span>}
            {proposal.status === 'applied' && <span className="badge ok">applied · created Plan v{nextVersion}</span>}
            {proposal.status === 'rejected' && <span className="badge impossible">rejected</span>}
            {isPending && <span className="badge">pending</span>}
          </div>
          <div className="prop-meta">
            Proposed by <b>{author}</b> · against Plan v{proposal.changeSet.basePlanVersion} · {createdLabel}
            {decider && (
              <>
                {' '}
                · {proposal.status === 'rejected' ? 'decided' : 'approved'} by <b>{decider}</b> (leader)
              </>
            )}
          </div>
        </div>
      </div>

      {isPending && <div className="prop-rat">{proposal.rationale}</div>}

      {detail && (
        <div>
          <span className="block-h">
            The change · {proposal.changeSet.ops.length} operation{proposal.changeSet.ops.length > 1 ? 's' : ''}
          </span>
          <ChangeList ops={proposal.changeSet.ops} detail={detail} extraPlaces={extraPlaces} className="prop-changes" />
        </div>
      )}

      {proposal.status === 'rejected' && proposal.rejectionReason && (
        <div className="rej-reason">
          <b>{decider}:</b> {proposal.rejectionReason}
        </div>
      )}

      {isPending && isLeader && !rejecting && (
        <div className="prop-actions">
          <button className="btn approve" disabled={approve.isPending} onClick={() => approve.mutate()}>
            Approve — applies as Plan v{nextVersion}
          </button>
          <button className="btn danger" onClick={() => setRejecting(true)}>
            Reject…
          </button>
          <button className="btn primary" disabled={toPoll.isPending} onClick={() => toPoll.mutate()}>
            Route to a poll
          </button>
          <span className="role-note">
            Approving writes Plan v{nextVersion} now and notifies the group. Old versions stay in history for rollback.
          </span>
        </div>
      )}

      {isPending && isLeader && rejecting && (
        <div className="reason">
          <label>Reject reason (shown to {author})</label>
          <textarea
            className="ta"
            rows={2}
            placeholder={`e.g. "Love the intent, but let's not overfill Day 5 — can we drop a stop instead of moving two?"`}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
          <div className="row">
            <button
              className="btn danger sm"
              disabled={!reason.trim() || reject.isPending}
              onClick={() => reject.mutate()}
            >
              Send rejection
            </button>
            <button className="btn sm" onClick={() => setRejecting(false)}>
              Cancel
            </button>
            <span className="hint">A reason is required so the proposer knows why.</span>
          </div>
        </div>
      )}

      {isPending && !isLeader && (
        <div className="status-strip locked">
          🔒{' '}
          <span>
            Awaiting a <b>leader's decision</b>. Members can't approve structural changes.
          </span>
        </div>
      )}

      {proposal.status === 'applied' && (
        <div className="decided">
          A leader's decision created Plan v{nextVersion}; the change is live on the Plan tab.
        </div>
      )}
    </div>
  );
}
