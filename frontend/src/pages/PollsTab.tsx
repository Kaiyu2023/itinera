import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/useApi';
import { invalidateTripPlanning } from '../api/queryInvalidation';
import { useMembers } from '../components/hooks';
import { SheetModal } from '../components/SheetModal';
import { ChangeList } from './governanceShared';
import { PollComposer } from './pollComposer';
import type { Place, Poll, PlanDetail, Proposal } from '../api/types';
import { fillStyle } from '../lib/oklch';
import { useOneShotDeepLink } from '../lib/useOneShotDeepLink';
import { useI18n } from '../i18n';

const POLL_STATUS_MESSAGE = {
  draft: 'polls.status.draft',
  scheduled: 'polls.status.scheduled',
  open: 'polls.status.open',
  passed: 'polls.status.passed',
  failed: 'polls.status.failed',
  expired: 'polls.status.expired',
} as const;

const MAX_BROWSER_TIMEOUT_MS = 2_147_483_647;

/** Force a render at the server-owned cutoff instead of waiting for another UI event. */
function useDeadlinePassed(closesAt: string): boolean {
  const deadline = Date.parse(closesAt);
  const [, rerender] = useState(0);
  useEffect(() => {
    if (!Number.isFinite(deadline) || deadline <= Date.now()) return;
    let timer: number | undefined;
    const schedule = () => {
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        rerender((value) => value + 1);
        return;
      }
      timer = window.setTimeout(schedule, Math.min(remaining, MAX_BROWSER_TIMEOUT_MS));
    };
    schedule();
    return () => window.clearTimeout(timer);
  }, [deadline]);
  return !Number.isFinite(deadline) || deadline <= Date.now();
}

function readNewPollDeepLink(params: URLSearchParams): true | null {
  return params.get('poll') === 'new' ? true : null;
}

function stripNewPollDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  next.delete('poll');
  return next;
}

/**
 * Governance home (DESIGN.md §4.2): proposals awaiting a decision, open polls
 * with live voting + quorum, the other lifecycle states, and the decided
 * history. Leaders get approve / reject / route-to-poll; members see status.
 */
export function PollsTab() {
  const { tripId } = useParams();
  const api = useApi();
  const { locale, t: ui } = useI18n();
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
  useOneShotDeepLink({
    ready: !!polls.data,
    searchParams: params,
    setSearchParams: setParams,
    read: readNewPollDeepLink,
    strip: stripNewPollDeepLink,
    onMatch: () => setComposing(true),
  });

  if (polls.isLoading || proposals.isLoading) return <p className="muted">{ui('polls.loading')}</p>;

  const currentRole = trip.data?.members.find((member) => member.userId === me.data?.id)?.role;
  const isLeader = currentRole === 'leader';
  const isEditor = currentRole === 'leader' || currentRole === 'member';
  const eligibleVoterCount = trip.data?.members.filter((member) => member.role !== 'viewer').length ?? 0;
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
        <h2>{ui('polls.title')}</h2>
        <span className="spacer" />
        {isEditor && (
          <button type="button" className="btn accent" onClick={() => setComposing(true)}>
            {ui('polls.new')}
          </button>
        )}
      </div>

      {pending.length > 0 && (
        <section className="gov-sec">
          <h3 className="gov-h">{ui('polls.section.awaiting')}</h3>
          {pending.map((p) => (
            <ProposalCard key={p.id} proposal={p} detail={detail} extraPlaces={extraPlaces} isLeader={isLeader} />
          ))}
        </section>
      )}

      <section className="gov-sec">
        <h3 className="gov-h">{ui('polls.section.open')}</h3>
        {open.length === 0 && (
          <div className="poll-zero">
            <strong>{ui('polls.empty.title')}</strong>
            <p className="muted">
              {ui('polls.empty.body', {
                quorum: trip.data
                  ? new Intl.NumberFormat(locale).format(Math.ceil(eligibleVoterCount / 2))
                  : ui('polls.empty.halfGroup'),
              })}
            </p>
            {isEditor && (
              <button type="button" className="btn primary sm" onClick={() => setComposing(true)}>
                {ui('polls.empty.start')}
              </button>
            )}
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
            isEditor={isEditor}
            meId={me.data?.id}
            memberCount={eligibleVoterCount}
            flashId={flashId}
          />
        ))}
      </section>

      {upcoming.length > 0 && (
        <section className="gov-sec">
          <h3 className="gov-h">{ui('polls.section.upcoming')}</h3>
          {upcoming.map((poll) => (
            <PollCard
              key={poll.id}
              poll={poll}
              detail={detail}
              proposals={proposals.data ?? []}
              extraPlaces={extraPlaces}
              isLeader={isLeader}
              isEditor={isEditor}
              meId={me.data?.id}
              memberCount={eligibleVoterCount}
              flashId={flashId}
            />
          ))}
        </section>
      )}

      {(decided.length > 0 || history.length > 0) && (
        <section className="gov-sec">
          <h3 className="gov-h">{ui('polls.section.decided')}</h3>
          {decided.map((poll) => (
            <PollCard
              key={poll.id}
              poll={poll}
              detail={detail}
              proposals={proposals.data ?? []}
              extraPlaces={extraPlaces}
              isLeader={isLeader}
              isEditor={isEditor}
              meId={me.data?.id}
              memberCount={eligibleVoterCount}
              flashId={flashId}
            />
          ))}
          {history.map((p) => (
            <ProposalCard key={p.id} proposal={p} detail={detail} extraPlaces={extraPlaces} isLeader={isLeader} />
          ))}
        </section>
      )}

      {composing && tripId && isEditor && (
        <PollComposer tripId={tripId} onCreated={setFlashId} onClose={() => setComposing(false)} />
      )}
    </div>
  );
}

/* ═══════════════ quorum meter ═══════════════ */

function QuorumMeter({ poll, memberCount }: { poll: Poll; memberCount: number }) {
  const { locale, t: ui } = useI18n();
  const formatNumber = (value: number) => new Intl.NumberFormat(locale).format(value);
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
        <b>{ui('polls.quorum.count', { voted: formatNumber(voted), members: formatNumber(memberCount) })}</b>{' '}
        {decided ? '' : `${ui('polls.quorum.voted')} `}·{' '}
        {ui('polls.quorum.label', { count: formatNumber(poll.quorum) })}{' '}
        {met ? (
          <span className="met">✓ {decided ? '' : ui('polls.quorum.met')}</span>
        ) : (
          <span className="nomet">✗ {ui('polls.quorum.notMet')}</span>
        )}
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
  isEditor,
  meId,
  memberCount,
  flashId,
}: {
  poll: Poll;
  detail: PlanDetail | null;
  proposals: Proposal[];
  extraPlaces: Place[];
  isLeader: boolean;
  isEditor: boolean;
  meId: string | undefined;
  memberCount: number;
  /** id of a just-created poll, briefly highlighted so the add is visible. */
  flashId?: string | null;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const members = useMembers(poll.tripId);
  const { locale, t: ui } = useI18n();
  const formatNumber = (value: number) => new Intl.NumberFormat(locale).format(value);
  const refresh = () => invalidateTripPlanning(queryClient, poll.tripId);
  const [confirmingClose, setConfirmingClose] = useState(false);

  const vote = useMutation({
    mutationFn: (optionIds: string[]) => api.vote(poll.tripId, poll.id, optionIds),
    onSuccess: refresh,
  });
  const openMut = useMutation({ mutationFn: () => api.openPoll(poll.tripId, poll.id), onSuccess: refresh });
  const closeMut = useMutation({
    mutationFn: () => api.closePoll(poll.tripId, poll.id),
    onSuccess: () => {
      setConfirmingClose(false);
      return refresh();
    },
  });

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
  const deadlinePassed = useDeadlinePassed(poll.closesAt);
  const acceptsVotes = isOpen && !deadlinePassed;
  const counts = new Map<string, number>();
  for (const v of poll.votes) counts.set(v.optionId, (counts.get(v.optionId) ?? 0) + 1);
  const maxCount = Math.max(1, ...counts.values());
  const winnerId = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
  const topCount = Math.max(0, ...counts.values());
  const tiedAtTop =
    topCount > 0 && poll.options.filter((option) => (counts.get(option.id) ?? 0) === topCount).length > 1;

  const changeProposal =
    poll.kind === 'plan_change'
      ? proposals.find((p) => p.id === poll.options.find((o) => o.proposalId)?.proposalId)
      : undefined;
  const planConflict = poll.status === 'failed' && (changeProposal?.status as string | undefined) === 'stale';
  const tiedDecision = poll.status === 'failed' && tiedAtTop;
  const statusMessage = planConflict
    ? ui('polls.status.planConflict')
    : tiedDecision
      ? ui('polls.status.noDecision')
      : ui(POLL_STATUS_MESSAGE[poll.status]);
  const statusClass = planConflict ? 'impossible' : tiedDecision ? 'tight' : statusBadge(poll.status);
  const isAuthor = poll.createdBy === meId;
  const canOpen = isEditor && !deadlinePassed && (isAuthor || isLeader);
  const authorName = members.byId.get(poll.createdBy)?.displayName ?? ui('polls.permission.authorFallback');
  const openPermission = deadlinePassed
    ? ui('polls.permission.deadlinePassed')
    : isAuthor
      ? ui('polls.permission.author')
      : isLeader
        ? ui('polls.permission.leader')
        : ui('polls.permission.blocked', { author: authorName });

  return (
    <>
      <div
        className={`card poll poll-${poll.status}${flashId === poll.id ? ' new-flash' : ''}`}
        data-flash-id={poll.id}
      >
        <div className="poll-top">
          <strong>{poll.title}</strong>
          <span className={`badge${poll.kind === 'plan_change' ? ' plan' : ''}`}>
            {ui(poll.kind === 'plan_change' ? 'polls.kind.planChange' : 'polls.kind.decision')}
          </span>
          <span className={`badge ${statusClass}`}>{statusMessage}</span>
          <span className="meta">
            {poll.status === 'scheduled' && poll.opensAt ? (
              ui('polls.meta.opens', { date: dayShort(poll.opensAt, locale) })
            ) : isOpen ? (
              <>
                {ui('polls.meta.closes', { date: dayTime(poll.closesAt, locale) })}
                <br />
                {timeLeftLabel(poll.closesAt, locale, ui)}
              </>
            ) : poll.status === 'expired' ? (
              ui('polls.meta.closed', { date: dayShort(decidedMoment(poll), locale) })
            ) : poll.status === 'passed' || poll.status === 'failed' ? (
              ui('polls.meta.decided', { date: dayShort(decidedMoment(poll), locale) })
            ) : null}
          </span>
        </div>
        {poll.description && <p className="poll-desc">{poll.description}</p>}
        <p className="poll-by">
          {ui('polls.openedBy')} <b>{members.byId.get(poll.createdBy)?.displayName ?? '—'}</b>
          {changeProposal && (
            <>
              {' '}
              ·{' '}
              {ui('polls.wrapsProposal', {
                proposal: changeProposal.id,
                version: formatNumber(changeProposal.changeSet.basePlanVersion),
              })}
            </>
          )}
        </p>

        {changeProposal && detail && (
          <div className="preview">
            <span className="block-h">
              {ui('polls.adoptingChange', {
                from: formatNumber(changeProposal.changeSet.basePlanVersion),
                to: formatNumber(changeProposal.changeSet.basePlanVersion + 1),
              })}
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
          role={acceptsVotes ? (poll.allowMulti ? 'group' : 'radiogroup') : undefined}
          aria-label={acceptsVotes ? poll.title : undefined}
        >
          {poll.options.map((option) => {
            const votersHere = poll.votes.filter((v) => v.optionId === option.id);
            const n = votersHere.length;
            const isMine = mine.has(option.id);
            const isWin = !isOpen && option.id === winnerId && poll.status === 'passed';
            const canVote = acceptsVotes && isEditor;
            return (
              <button
                key={option.id}
                type="button"
                role={acceptsVotes ? (poll.allowMulti ? 'checkbox' : 'radio') : undefined}
                aria-checked={acceptsVotes ? isMine : undefined}
                /* Without this the accessible name was the button's whole text
                 content — the label, then the avatar initials, then a bare
                 number: "Ichiran (ramen, solo booths) R F 2". */
                aria-label={ui('polls.optionAria', {
                  option: option.label,
                  winner: isWin ? ui('polls.optionWinnerAria') : '',
                  votes: ui(n === 1 ? 'polls.vote.one' : 'polls.vote.many', { count: formatNumber(n) }),
                })}
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
                  {isMine && <span className="yours">{ui('polls.yourVote')}</span>}
                  {isWin && <span className="badge ok">{ui('polls.winner')}</span>}
                </span>
                <span className="voters" aria-hidden="true">
                  {isMine && <span className="tick">✓</span>}
                  <span className="stack">
                    {votersHere.map((v) => {
                      const u = members.byId.get(v.userId);
                      return u ? (
                        <span
                          key={v.userId}
                          className="avatar sm"
                          style={fillStyle(u.avatarColor)}
                          title={u.displayName}
                        >
                          {u.displayName[0]}
                        </span>
                      ) : null;
                    })}
                  </span>
                  <span className="cnt">{formatNumber(n)}</span>
                </span>
              </button>
            );
          })}
        </div>

        {poll.status !== 'draft' && poll.status !== 'scheduled' && (
          <QuorumMeter poll={poll} memberCount={memberCount} />
        )}

        {poll.resolutionNote && (
          <p className={`poll-resolution${planConflict ? ' conflict' : tiedDecision ? ' no-decision' : ''}`}>
            {(planConflict || tiedDecision) && (
              <strong>{ui(planConflict ? 'polls.resolution.conflict' : 'polls.resolution.noDecision')}</strong>
            )}
            <span>{poll.resolutionNote}</span>
          </p>
        )}

        <div className="poll-foot">
          {poll.status === 'draft' && (
            <>
              <span className="poll-open-copy">
                <span className="hint">{ui('polls.draftHint')}</span>
                <span className={`poll-permission${canOpen ? '' : ' locked'}`}>{openPermission}</span>
              </span>
              <span className="spacer" />
              {canOpen && (
                <button
                  type="button"
                  className="btn solid sm"
                  disabled={openMut.isPending}
                  onClick={() => openMut.mutate()}
                >
                  {ui('polls.openPoll')}
                </button>
              )}
            </>
          )}
          {poll.status === 'scheduled' && (
            <>
              <span className="poll-open-copy">
                <span className="hint">{ui('polls.scheduledHint')}</span>
                <span className={`poll-permission${canOpen ? '' : ' locked'}`}>{openPermission}</span>
              </span>
              <span className="spacer" />
              {canOpen && (
                <button
                  type="button"
                  className="btn primary sm"
                  disabled={openMut.isPending}
                  onClick={() => openMut.mutate()}
                >
                  {ui('polls.openNow')}
                </button>
              )}
            </>
          )}
          {isOpen && (
            <>
              {deadlinePassed ? (
                <span className="prompt">{ui('polls.votingClosed')}</span>
              ) : myVotes.length > 0 ? (
                <span className="prompt">
                  {ui('polls.youVoted')}{' '}
                  <b>{myVotes.map((id) => poll.options.find((o) => o.id === id)?.label ?? '—').join(', ')}</b>.{' '}
                  {ui(poll.allowMulti ? 'polls.multiChangeVote' : 'polls.singleChangeVote')}
                </span>
              ) : (
                <>
                  <span className="prompt">{ui('polls.notVoted')}</span>
                  <span className="hint">{ui(poll.allowMulti ? 'polls.multiVoteHint' : 'polls.singleVoteHint')}</span>
                </>
              )}
              <span className="spacer" />
              {poll.kind === 'plan_change' && (
                <span className="hint">
                  {ui('polls.passesAsPlan', {
                    version: formatNumber((changeProposal?.changeSet.basePlanVersion ?? 3) + 1),
                  })}
                </span>
              )}
              {isLeader && (
                <button
                  type="button"
                  className="btn ghost-link sm"
                  aria-haspopup="dialog"
                  disabled={closeMut.isPending}
                  onClick={() => setConfirmingClose(true)}
                >
                  {ui('polls.closeNow')}
                </button>
              )}
            </>
          )}
        </div>
      </div>
      {confirmingClose && (
        <ClosePollDialog
          poll={poll}
          proposal={changeProposal}
          detail={detail}
          memberCount={memberCount}
          busy={closeMut.isPending}
          failed={closeMut.isError}
          onClose={() => !closeMut.isPending && setConfirmingClose(false)}
          onConfirm={() => closeMut.mutate()}
        />
      )}
    </>
  );
}

/**
 * Closing is terminal, and for a plan-change poll it can publish a new plan
 * immediately. The old one-click text link hid both facts. This confirmation
 * makes the live tally, quorum and exact plan-version consequence inspectable
 * before the leader ends voting.
 */
function ClosePollDialog({
  poll,
  proposal,
  detail,
  memberCount,
  busy,
  failed,
  onClose,
  onConfirm,
}: {
  poll: Poll;
  proposal: Proposal | undefined;
  detail: PlanDetail | null;
  memberCount: number;
  busy: boolean;
  failed: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { locale, t: ui } = useI18n();
  const number = (value: number) => new Intl.NumberFormat(locale).format(value);
  const voters = new Set(poll.votes.map((vote) => vote.userId)).size;
  const counts = new Map<string, number>();
  for (const vote of poll.votes) counts.set(vote.optionId, (counts.get(vote.optionId) ?? 0) + 1);
  const topCount = Math.max(0, ...counts.values());
  const leaders = poll.options.filter((option) => topCount > 0 && (counts.get(option.id) ?? 0) === topCount);
  const winner = leaders.length === 1 ? leaders[0] : undefined;
  const tied = leaders.length > 1;
  const quorumMet = voters >= poll.quorum;
  const currentVersion = detail?.plan.version;
  const baseVersion = proposal?.changeSet.basePlanVersion;
  const stale =
    (proposal?.status as string | undefined) === 'stale' ||
    (currentVersion !== undefined && baseVersion !== undefined && currentVersion !== baseVersion);
  const planReady = poll.kind !== 'plan_change' || (!!detail && !!proposal);
  const publishesPlan =
    poll.kind === 'plan_change' && quorumMet && !tied && !stale && !!winner?.proposalId && planReady;
  const closesWithoutDecision = !quorumMet || tied || stale;

  const currentResult =
    topCount === 0
      ? ui('polls.close.result.none')
      : tied
        ? ui('polls.close.result.tie', { count: number(topCount) })
        : ui('polls.close.result.leader', { option: winner?.label ?? '—', count: number(topCount) });

  let consequence: string;
  let consequenceTone = 'no-decision';
  if (!planReady) {
    consequence = ui('polls.close.consequence.loadingPlan');
    consequenceTone = 'pending';
  } else if (poll.kind === 'plan_change' && stale) {
    consequence = ui('polls.close.consequence.stalePlan', {
      base: number(baseVersion ?? 0),
      current: number(currentVersion ?? 0),
    });
    consequenceTone = 'conflict';
  } else if (!quorumMet) {
    consequence =
      poll.kind === 'plan_change'
        ? ui('polls.close.consequence.noQuorumPlan', { current: number(currentVersion ?? 0) })
        : ui('polls.close.consequence.noQuorum');
  } else if (tied) {
    consequence =
      poll.kind === 'plan_change'
        ? ui('polls.close.consequence.tiePlan', { current: number(currentVersion ?? 0) })
        : ui('polls.close.consequence.tie');
  } else if (publishesPlan) {
    consequence = ui('polls.close.consequence.publishPlan', {
      current: number(currentVersion ?? 0),
      next: number((baseVersion ?? 0) + 1),
    });
    consequenceTone = 'apply';
  } else if (poll.kind === 'plan_change') {
    consequence = ui('polls.close.consequence.keepPlan', { current: number(currentVersion ?? 0) });
  } else {
    consequence = ui('polls.close.consequence.recordDecision', { option: winner?.label ?? '—' });
    consequenceTone = 'record';
  }

  const confirmLabel = publishesPlan
    ? ui('polls.close.confirmPublish', { version: number((baseVersion ?? 0) + 1) })
    : closesWithoutDecision
      ? ui('polls.close.confirmNoDecision')
      : ui('polls.close.confirmResult');
  const titleId = `poll-close-${poll.id}-title`;
  const copyId = `poll-close-${poll.id}-copy`;

  return (
    <SheetModal onClose={onClose}>
      <div
        className="exp-modal poll-close-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={copyId}
        aria-busy={busy}
      >
        <span className="poll-close-grip" aria-hidden />
        <header className="poll-close-head">
          <span className="poll-close-mark" aria-hidden>
            <svg viewBox="0 0 24 24">
              <circle cx="12" cy="12" r="8" />
              <path d="M12 7.5v5l3 2" />
            </svg>
          </span>
          <span className="poll-close-title">
            <span>{ui('polls.close.eyebrow')}</span>
            <h2 id={titleId}>{ui('polls.close.title', { poll: poll.title })}</h2>
          </span>
          <button
            type="button"
            className="x poll-close-x"
            onClick={onClose}
            disabled={busy}
            aria-label={ui('common.close')}
          >
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d="m6 6 12 12M18 6 6 18" />
            </svg>
          </button>
        </header>

        <div className="poll-close-body" id={copyId}>
          <p className="poll-close-intro">{ui('polls.close.intro')}</p>
          <dl className="poll-close-stats">
            <div>
              <dt>{ui('polls.close.participation')}</dt>
              <dd>{ui('polls.close.votedCount', { voted: number(voters), members: number(memberCount) })}</dd>
            </div>
            <div className={quorumMet ? 'met' : 'not-met'}>
              <dt>{ui('polls.close.quorum')}</dt>
              <dd>
                {ui('polls.close.quorumCount', {
                  quorum: number(poll.quorum),
                  status: ui(quorumMet ? 'polls.quorum.met' : 'polls.quorum.notMet'),
                })}
              </dd>
            </div>
          </dl>
          <div className="poll-close-result">
            <span>{ui('polls.close.currentResult')}</span>
            <strong>{currentResult}</strong>
          </div>
          <div className={`poll-close-consequence ${consequenceTone}`}>
            <svg viewBox="0 0 24 24" aria-hidden>
              {consequenceTone === 'apply' ? (
                <path d="m5 12 4 4L19 6" />
              ) : consequenceTone === 'conflict' ? (
                <path d="M12 8v5M12 16.5v.1M12 3.5 21 20H3Z" />
              ) : (
                <path d="M5 12h14M12 5v14" />
              )}
            </svg>
            <span>
              <strong>{ui('polls.close.whatHappens')}</strong>
              {consequence}
            </span>
          </div>
          {failed && (
            <p className="poll-close-error" role="alert">
              {ui('polls.close.error')}
            </p>
          )}
        </div>

        <footer className="poll-close-actions">
          <button type="button" className="btn poll-close-cancel" onClick={onClose} disabled={busy}>
            {ui('polls.close.cancel')}
          </button>
          <button
            type="button"
            className="btn danger poll-close-confirm"
            onClick={onConfirm}
            disabled={busy || !planReady}
          >
            {confirmLabel}
          </button>
        </footer>
      </div>
    </SheetModal>
  );
}

/** "Fri 24 Jul" — the app's compact date, matching the mockup's poll meta. */
function dayShort(iso: string, locale: ReturnType<typeof useI18n>['locale']): string {
  return new Intl.DateTimeFormat(locale, { weekday: 'short', day: 'numeric', month: 'short' }).format(new Date(iso));
}

/** "Sat 25 Jul, 13:00" — date + local time, for an open poll's close moment. */
function dayTime(iso: string, locale: ReturnType<typeof useI18n>['locale']): string {
  return new Intl.DateTimeFormat(locale, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(iso));
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
function timeLeftLabel(
  closesAt: string,
  locale: ReturnType<typeof useI18n>['locale'],
  ui: ReturnType<typeof useI18n>['t'],
): string {
  const ms = new Date(closesAt).getTime() - Date.now();
  if (ms <= 0) return ui('polls.time.past');
  const days = Math.floor(ms / 86_400_000);
  const formatNumber = (value: number) => new Intl.NumberFormat(locale).format(value);
  if (days === 0) {
    return ms < 3_600_000
      ? ui('polls.time.withinHour')
      : ui('polls.time.hours', { count: formatNumber(Math.floor(ms / 3_600_000)) });
  }
  return ui(days === 1 ? 'polls.time.day' : 'polls.time.days', { count: formatNumber(days) });
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
  const { locale, t: ui } = useI18n();
  const formatNumber = (value: number) => new Intl.NumberFormat(locale).format(value);
  const refresh = () => invalidateTripPlanning(queryClient, proposal.tripId);
  const [rejecting, setRejecting] = useState(false);
  const [reason, setReason] = useState('');

  const approve = useMutation({
    mutationFn: () => api.approveProposal(proposal.tripId, proposal.id),
    onSuccess: refresh,
  });
  const reject = useMutation({
    mutationFn: () => api.rejectProposal(proposal.tripId, proposal.id, reason),
    onSuccess: () => {
      setRejecting(false);
      refresh();
    },
  });
  const toPoll = useMutation({
    mutationFn: () => api.proposalToPoll(proposal.tripId, proposal.id),
    onSuccess: refresh,
  });

  const author = members.byId.get(proposal.createdBy)?.displayName ?? '—';
  const nextVersion = proposal.changeSet.basePlanVersion + 1;
  const decider =
    proposal.decidedBy?.kind === 'leader' ? members.byId.get(proposal.decidedBy.userId)?.displayName : undefined;
  const isPending = proposal.status === 'pending';
  const createdLabel = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short' }).format(
    new Date(proposal.createdAt),
  );

  return (
    <div className="card prop">
      <div className="prop-head">
        <span className="avatar" style={fillStyle(members.byId.get(proposal.createdBy)?.avatarColor ?? '#888')}>
          {author[0]}
        </span>
        <div className="ti">
          <strong>{proposal.title}</strong>
          <div className="tags">
            <span className="badge">{ui('proposals.badge.structural')}</span>
            {isPending && isLeader && <span className="badge tight">{ui('proposals.badge.needsDecision')}</span>}
            {proposal.status === 'applied' && (
              <span className="badge ok">{ui('proposals.badge.applied', { version: formatNumber(nextVersion) })}</span>
            )}
            {proposal.status === 'rejected' && (
              <span className="badge impossible">{ui('proposals.badge.rejected')}</span>
            )}
            {isPending && <span className="badge">{ui('proposals.badge.pending')}</span>}
          </div>
          <div className="prop-meta">
            {ui('proposals.meta.proposedBy')} <b>{author}</b> ·{' '}
            {ui('proposals.meta.againstPlan', { version: formatNumber(proposal.changeSet.basePlanVersion) })} ·{' '}
            {createdLabel}
            {decider && (
              <>
                {' '}
                · {ui(proposal.status === 'rejected' ? 'proposals.meta.decidedBy' : 'proposals.meta.approvedBy')}{' '}
                <b>{decider}</b> ({ui('proposals.meta.leader')})
              </>
            )}
          </div>
        </div>
      </div>

      {isPending && <div className="prop-rat">{proposal.rationale}</div>}

      {detail && (
        <div>
          <span className="block-h">
            {ui(proposal.changeSet.ops.length === 1 ? 'proposals.change.one' : 'proposals.change.many', {
              count: formatNumber(proposal.changeSet.ops.length),
            })}
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
          <button type="button" className="btn approve" disabled={approve.isPending} onClick={() => approve.mutate()}>
            {ui('proposals.approve', { version: formatNumber(nextVersion) })}
          </button>
          <button type="button" className="btn danger" onClick={() => setRejecting(true)}>
            {ui('proposals.reject')}
          </button>
          <button type="button" className="btn primary" disabled={toPoll.isPending} onClick={() => toPoll.mutate()}>
            {ui('proposals.routeToPoll')}
          </button>
          <span className="role-note">{ui('proposals.approveHint', { version: formatNumber(nextVersion) })}</span>
        </div>
      )}

      {isPending && isLeader && rejecting && (
        <div className="reason">
          <label>{ui('proposals.rejectReason', { name: author })}</label>
          <textarea
            className="ta"
            rows={2}
            placeholder={ui('proposals.rejectPlaceholder')}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
          <div className="row">
            <button
              type="button"
              className="btn danger sm"
              disabled={!reason.trim() || reject.isPending}
              onClick={() => reject.mutate()}
            >
              {ui('proposals.sendRejection')}
            </button>
            <button type="button" className="btn sm" onClick={() => setRejecting(false)}>
              {ui('common.cancel')}
            </button>
            <span className="hint">{ui('proposals.reasonRequired')}</span>
          </div>
        </div>
      )}

      {isPending && !isLeader && (
        <div className="status-strip locked">
          🔒{' '}
          <span>
            {ui('proposals.awaitingLeaderPrefix')} <b>{ui('proposals.awaitingLeaderDecision')}</b>.{' '}
            {ui('proposals.awaitingLeaderSuffix')}
          </span>
        </div>
      )}

      {proposal.status === 'applied' && (
        <div className="decided">{ui('proposals.appliedNotice', { version: formatNumber(nextVersion) })}</div>
      )}
    </div>
  );
}
