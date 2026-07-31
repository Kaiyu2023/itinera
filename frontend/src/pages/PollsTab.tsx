import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { ChangeList } from './governanceShared';
import { PollComposer } from './pollComposer';
import type { Place, Poll, PlanDetail, Proposal } from '../api/types';
import { fillStyle } from '../lib/oklch';

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
  // A freshly created poll sorts into whichever section it belongs to, which is
  // usually below the fold — the composer closed and, as far as the eye could
  // tell, nothing happened. Scroll the new card into view and flash it.
  const [flashId, setFlashId] = useState<string | null>(null);
  useEffect(() => {
    if (!flashId) return;
    let raf = 0;
    let tries = 0;
    // The list is still refetching when the composer closes, so poll for the
    // card rather than assuming it is already mounted.
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
        {/* <h2>, not <h1>: TripLayout's hero already gives the page its single
            <h1> (the trip name), and a second one per tab left the document
            with two competing top-level headings. */}
        <h2>Governance</h2>
        <span className="spacer" />
        <button type="button" className="btn accent" onClick={() => setComposing(true)}>
          ＋ New poll
        </button>
      </div>

      {pending.length > 0 && (
        <section className="gov-sec">
          <h3 className="gov-h">Awaiting a decision</h3>
          {pending.map((p) => (
            <ProposalCard key={p.id} proposal={p} detail={detail} extraPlaces={extraPlaces} isLeader={isLeader} />
          ))}
        </section>
      )}

      <section className="gov-sec">
        <h3 className="gov-h">Open polls</h3>
        {open.length === 0 && (
          <div className="poll-zero">
            <strong>Nothing to vote on right now.</strong>
            <p className="muted">
              A poll is how the group settles a choice nobody should make alone — which dinner, which day-trip, whether
              to adopt a change someone proposed to the plan. Everyone votes, votes stay changeable until it closes, and
              it only counts once {trip.data ? Math.ceil(trip.data.members.length / 2) : 'half the group'} of you have
              weighed in.
            </p>
            <button type="button" className="btn primary sm" onClick={() => setComposing(true)}>
              Start one →
            </button>
          </div>
        )}
        {open.map((poll) => (
          <PollCard
            key={poll.id}
            poll={poll}
            detail={detail}
            proposals={proposals.data ?? []}
            extraPlaces={extraPlaces}
            isLeader={isLeader}
            meId={me.data?.id}
            flashId={flashId}
          />
        ))}
      </section>

      {upcoming.length > 0 && (
        <section className="gov-sec">
          <h3 className="gov-h">Drafts &amp; scheduled</h3>
          {upcoming.map((poll) => (
            <PollCard
              key={poll.id}
              poll={poll}
              detail={detail}
              proposals={proposals.data ?? []}
              extraPlaces={extraPlaces}
              isLeader={isLeader}
              meId={me.data?.id}
              flashId={flashId}
            />
          ))}
        </section>
      )}

      {(decided.length > 0 || history.length > 0) && (
        <section className="gov-sec">
          <h3 className="gov-h">Decided</h3>
          {decided.map((poll) => (
            <PollCard
              key={poll.id}
              poll={poll}
              detail={detail}
              proposals={proposals.data ?? []}
              extraPlaces={extraPlaces}
              isLeader={isLeader}
              meId={me.data?.id}
              flashId={flashId}
            />
          ))}
          {history.map((p) => (
            <ProposalCard key={p.id} proposal={p} detail={detail} extraPlaces={extraPlaces} isLeader={isLeader} />
          ))}
        </section>
      )}

      {composing && tripId && (
        <PollComposer tripId={tripId} onCreated={setFlashId} onClose={() => setComposing(false)} />
      )}
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
      {/* Decorative: the sentence to the right already states the counts, and
          the pips announced as a run of empty elements. */}
      <span className="pips" aria-hidden="true">
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
  flashId,
}: {
  poll: Poll;
  detail: PlanDetail | null;
  proposals: Proposal[];
  extraPlaces: Place[];
  isLeader: boolean;
  meId: string | undefined;
  /** id of a just-created poll, briefly highlighted so the add is visible. */
  flashId?: string | null;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const members = useMembers(poll.tripId);
  const refresh = () => queryClient.invalidateQueries();

  const vote = useMutation({ mutationFn: (optionIds: string[]) => api.vote(poll.id, optionIds), onSuccess: refresh });
  const openMut = useMutation({ mutationFn: () => api.openPoll(poll.id), onSuccess: refresh });
  const closeMut = useMutation({ mutationFn: () => api.closePoll(poll.id), onSuccess: refresh });

  /**
   * `allowMulti` was written by the composer, stored on the poll, and then read
   * by nobody: every click sent `[optionId]`, and the port replaces the voter's
   * whole ballot on each call, so a multi-choice poll behaved exactly like a
   * single-choice one. Now a multi poll toggles the clicked option into or out
   * of the ballot and sends the whole set; an empty set is a legitimate "I've
   * withdrawn my vote".
   */
  const myVotes = poll.votes.filter((v) => v.userId === meId).map((v) => v.optionId);
  const mine = new Set(myVotes);
  const castVote = (optionId: string) => {
    if (!poll.allowMulti) return vote.mutate([optionId]);
    vote.mutate(mine.has(optionId) ? myVotes.filter((id) => id !== optionId) : [...myVotes, optionId]);
  };

  const isOpen = poll.status === 'open';
  const counts = new Map<string, number>();
  for (const v of poll.votes) counts.set(v.optionId, (counts.get(v.optionId) ?? 0) + 1);
  const maxCount = Math.max(1, ...counts.values());
  const winnerId = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];

  const changeProposal =
    poll.kind === 'plan_change'
      ? proposals.find((p) => p.id === poll.options.find((o) => o.proposalId)?.proposalId)
      : undefined;

  return (
    <div className={`card poll poll-${poll.status}${flashId === poll.id ? ' new-flash' : ''}`} data-flash-id={poll.id}>
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
              {timeLeftLabel(poll.closesAt)}
            </>
          ) : poll.status === 'expired' ? (
            `closed ${dayShort(decidedMoment(poll))}`
          ) : poll.status === 'passed' || poll.status === 'failed' ? (
            `decided ${dayShort(decidedMoment(poll))}`
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

      {/* An open poll's options are a live choice, so they carry radio (single
          choice) or checkbox (allowMulti) semantics and announce "your vote" as
          state rather than as trailing text. A decided poll's options are just
          a result readout — plain disabled buttons, no group. */}
      <div
        className="poll-opts"
        role={isOpen ? (poll.allowMulti ? 'group' : 'radiogroup') : undefined}
        aria-label={isOpen ? poll.title : undefined}
      >
        {poll.options.map((option) => {
          const votersHere = poll.votes.filter((v) => v.optionId === option.id);
          const n = votersHere.length;
          const isMine = mine.has(option.id);
          const isWin = !isOpen && option.id === winnerId && poll.status === 'passed';
          const canVote = isOpen;
          return (
            <button
              key={option.id}
              type="button"
              role={isOpen ? (poll.allowMulti ? 'checkbox' : 'radio') : undefined}
              aria-checked={isOpen ? isMine : undefined}
              /* Without this the accessible name was the button's whole text
                 content — the label, then the avatar initials, then a bare
                 number: "Ichiran (ramen, solo booths) R F 2". */
              aria-label={`${option.label}${isWin ? ' — winner' : ''} — ${n} ${n === 1 ? 'vote' : 'votes'}`}
              className={`opt${isMine ? ' mine' : ''}${isWin ? ' win' : ''}`}
              disabled={!canVote || vote.isPending}
              onClick={() => canVote && castVote(option.id)}
            >
              {/* The 8% floor exists so a single vote against a landslide still
                  draws something you can see. It used to be applied before the
                  zero check, so every option nobody had voted for drew the same
                  8% stub and read as "a few votes". No votes, no bar. */}
              <span className="fill" style={{ width: n === 0 ? 0 : `${Math.max(8, (n / maxCount) * 100)}%` }} />
              <span className="lab">
                {option.label}
                {isMine && <span className="yours">· your vote</span>}
                {isWin && <span className="badge ok">winner</span>}
              </span>
              <span className="voters" aria-hidden="true">
                {isMine && <span className="tick">✓</span>}
                <span className="stack">
                  {votersHere.map((v) => {
                    const u = members.byId.get(v.userId);
                    return u ? (
                      <span key={v.userId} className="avatar sm" style={fillStyle(u.avatarColor)} title={u.displayName}>
                        {u.displayName[0]}
                      </span>
                    ) : null;
                  })}
                </span>
                <span className="cnt">{n}</span>
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
            {myVotes.length > 0 ? (
              <span className="prompt">
                You voted{' '}
                <b>
                  {myVotes.map((id) => poll.options.find((o) => o.id === id)?.label.split(' (')[0] ?? '—').join(', ')}
                </b>
                . {poll.allowMulti ? 'Tap any option to add or drop it.' : 'Tap another option to change it.'}
              </span>
            ) : (
              <>
                <span className="prompt">You haven't voted —</span>
                <span className="hint">
                  {poll.allowMulti
                    ? 'pick as many options as you like. Changeable until close.'
                    : 'tap an option to cast. Changeable until close.'}
                </span>
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

/**
 * When a decided poll actually stopped taking votes.
 *
 * `closesAt` is only the *scheduled* deadline. A leader hitting "Close now"
 * ends the poll before it, so printing `closesAt` stamped polls decided today
 * with tomorrow's date — "closed Sun 2 Aug" on 30 Jul. Records written before
 * `decidedAt` existed have no honest answer, so they clamp: a poll cannot have
 * been decided in the future.
 */
function decidedMoment(poll: Poll): string {
  if (poll.decidedAt) return poll.decidedAt;
  return new Date(Math.min(new Date(poll.closesAt).getTime(), Date.now())).toISOString();
}

/**
 * How long an open poll has left.
 *
 * Was `Math.max(0, Math.ceil(ms / day))`, which broke at both ends: `ceil`
 * turned the last four hours of a poll into "1 day left", and the `max(0, …)`
 * turned "closed six hours ago, nobody has pressed Close" into the same
 * "0 days left" as "closes tonight". Floor tells the truth about whole days
 * remaining, and the two edge cases get to say what they actually mean.
 */
function timeLeftLabel(closesAt: string): string {
  const ms = new Date(closesAt).getTime() - Date.now();
  if (ms <= 0) return 'past its close time';
  const days = Math.floor(ms / 86_400_000);
  if (days === 0) return ms < 3_600_000 ? 'closes within the hour' : `closes in ${Math.floor(ms / 3_600_000)} h`;
  return `${days} ${days === 1 ? 'day' : 'days'} left`;
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
        <span className="avatar" style={fillStyle(members.byId.get(proposal.createdBy)?.avatarColor ?? '#888')}>
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
