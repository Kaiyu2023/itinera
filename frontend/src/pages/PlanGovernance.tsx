import { createContext, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { useIsDesktop } from '../components/hooks';
import { KIND_COLOR, PLACE_KIND_COLOR } from './planShared';
import { ChangeList, PLACE_TO_STOP_KIND, dayOptionLabel } from './governanceShared';
import type { CandidateWithPlace, ChangeOp, Day, PlanDetail, ProposalRoute, Stop, Thread, User } from '../api/types';

/**
 * Wiring for the three Plan-tab stop actions (§ mockup d). A small context lets
 * the deeply-nested popover / sheet / panel buttons open one of three surfaces:
 * the Discuss thread, the Propose-change composer, or the Propose-a-stop
 * composer. Desktop renders them as a centered modal; mobile as a bottom sheet.
 */

type GovAction =
  | { kind: 'discuss'; stop: Stop }
  | { kind: 'change'; stop: Stop }
  | { kind: 'addStop'; day: Day };

interface PlanActions {
  discuss: (stop: Stop) => void;
  proposeChange: (stop: Stop) => void;
  proposeStop: (day: Day) => void;
}

const PlanActionsContext = createContext<PlanActions | null>(null);
const NOOP: PlanActions = { discuss: () => {}, proposeChange: () => {}, proposeStop: () => {} };
export function usePlanActions(): PlanActions {
  return useContext(PlanActionsContext) ?? NOOP;
}

export interface PlanGovernanceProps {
  tripId: string;
  detail: PlanDetail;
  days: Day[];
  candidates: CandidateWithPlace[];
  membersById: Map<string, User>;
  threads: Thread[];
  children: ReactNode;
}

export function PlanGovernanceProvider({ tripId, detail, days, candidates, membersById, threads, children }: PlanGovernanceProps) {
  const [action, setAction] = useState<GovAction | null>(null);
  const actions = useMemo<PlanActions>(
    () => ({
      discuss: (stop) => setAction({ kind: 'discuss', stop }),
      proposeChange: (stop) => setAction({ kind: 'change', stop }),
      proposeStop: (day) => setAction({ kind: 'addStop', day }),
    }),
    [],
  );
  const close = () => setAction(null);

  return (
    <PlanActionsContext.Provider value={actions}>
      {children}
      {action && (
        <GovModal onClose={close}>
          {action.kind === 'discuss' && (
            <ThreadPanel stop={action.stop} detail={detail} threads={threads} membersById={membersById} onClose={close} />
          )}
          {action.kind === 'change' && <ProposeChange stop={action.stop} detail={detail} days={days} tripId={tripId} onClose={close} />}
          {action.kind === 'addStop' && <ProposeStop day={action.day} detail={detail} candidates={candidates} tripId={tripId} onClose={close} />}
        </GovModal>
      )}
    </PlanActionsContext.Provider>
  );
}

/* ═══════════════ modal chrome ═══════════════ */

function GovModal({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  const isDesktop = useIsDesktop();
  return (
    <div className="gov-backdrop" onClick={onClose}>
      <div className={`gov-modal${isDesktop ? '' : ' sheet'}`} onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
        {!isDesktop && <div className="gov-grip"><span /></div>}
        {children}
      </div>
    </div>
  );
}

/** Minimal inline emphasis for comment bodies: **bold** and *italic* only,
    rendered as safe React elements (no dangerouslySetInnerHTML). */
function renderEmphasis(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const re = /\*\*([^*]+)\*\*|\*([^*]+)\*/g;
  let last = 0;
  let key = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (m.index > last) nodes.push(text.slice(last, m.index));
    if (m[1] != null) nodes.push(<strong key={key++}>{m[1]}</strong>);
    else nodes.push(<em key={key++}>{m[2]}</em>);
    last = re.lastIndex;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

/* ═══════════════ Discuss thread ═══════════════ */

function ThreadPanel({
  stop,
  detail,
  threads,
  membersById,
  onClose,
}: {
  stop: Stop;
  detail: PlanDetail;
  threads: Thread[];
  membersById: Map<string, User>;
  onClose: () => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const place = detail.places.find((p) => p.id === stop.placeId);
  const dayIndex = [...detail.days].sort((a, b) => a.date.localeCompare(b.date)).findIndex((d) => d.id === stop.dayId);
  const thread = threads.find((t) => t.anchor.kind === 'stop' && t.anchor.stopId === stop.id);
  const [draft, setDraft] = useState('');

  const comments = useQuery({ queryKey: ['comments', thread?.id], queryFn: () => api.getComments(thread!.id), enabled: !!thread });
  const post = useMutation({
    mutationFn: (body: string) => api.addComment(thread!.id, body),
    onSuccess: () => { setDraft(''); queryClient.invalidateQueries({ queryKey: ['comments', thread?.id] }); queryClient.invalidateQueries({ queryKey: ['threads'] }); },
  });
  const react = useMutation({
    mutationFn: ({ commentId, emoji }: { commentId: string; emoji: string }) => api.toggleReaction(commentId, emoji),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['comments', thread?.id] }),
  });

  return (
    <div className="panel-card">
      <div className="panel-top">
        <span className="anchor"><span className="kd" style={{ background: KIND_COLOR[stop.stopKind] }} />{place?.name} · Day {dayIndex + 1}</span>
        <button type="button" className="close" onClick={onClose} aria-label="Close">✕</button>
      </div>
      {thread ? (
        <>
          <div className="thread-title">{thread.title}</div>
          <div className="thread-body">
            {comments.isLoading && <p className="muted">Loading…</p>}
            {(comments.data ?? []).map((c) => {
              const author = membersById.get(c.author);
              const mine = c.author === me.data?.id;
              return (
                <div key={c.id} className={`cmt${mine ? ' me' : ''}`}>
                  <span className="avatar sm" style={{ background: author?.avatarColor ?? '#888' }}>{author?.displayName[0] ?? '?'}</span>
                  <div>
                    <div className="bubble">
                      <div className="ch"><span className="nm">{author?.displayName ?? '—'}</span><span className="tm">{new Date(c.createdAt).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}</span></div>
                      <div className="bd">{renderEmphasis(c.body)}</div>
                    </div>
                    <div className="rxn">
                      {c.reactions.map((r) => (
                        <button
                          key={r.emoji}
                          type="button"
                          className={`r${r.userIds.includes(me.data?.id ?? '') ? ' on' : ''}`}
                          onClick={() => react.mutate({ commentId: c.id, emoji: r.emoji })}
                        >
                          {r.emoji} {r.userIds.length}
                        </button>
                      ))}
                      <button type="button" className="r add" onClick={() => react.mutate({ commentId: c.id, emoji: '👍' })}>+👍</button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <form
            className="composer"
            onSubmit={(e) => { e.preventDefault(); if (draft.trim()) post.mutate(draft.trim()); }}
          >
            <span className="avatar sm" style={{ background: me.data?.avatarColor ?? '#6b5bd2' }}>{me.data?.displayName[0] ?? 'K'}</span>
            <input className="in" placeholder="Add to the thread…" value={draft} onChange={(e) => setDraft(e.target.value)} />
            <button className="btn solid sm" type="submit" disabled={!draft.trim() || post.isPending}>Send</button>
          </form>
        </>
      ) : (
        <div className="thread-body">
          <p className="muted">No discussion on this stop yet. Threads seed from the first comment — the group can start one from the notices for now.</p>
        </div>
      )}
    </div>
  );
}

/* ═══════════════ Propose a change ═══════════════ */

function RouteSeg({ value, onChange }: { value: ProposalRoute; onChange: (r: ProposalRoute) => void }) {
  return (
    <div style={{ display: 'flex', gap: 10, alignItems: 'center', flexWrap: 'wrap' }}>
      <span className="fl">Route</span>
      <span className="route-seg">
        <button type="button" className={value === 'leader_approval' ? 'active' : ''} onClick={() => onChange('leader_approval')}>Request a leader's approval</button>
        <button type="button" className={value === 'poll' ? 'active' : ''} onClick={() => onChange('poll')}>Open a poll</button>
      </span>
    </div>
  );
}

function Sent({ route, onClose }: { route: ProposalRoute; onClose: () => void }) {
  return (
    <div className="compose sent">
      <strong>Sent to leaders ✓</strong>
      <p className="muted">
        {route === 'poll' ? 'A poll is opening for the group to decide.' : 'A leader will approve or reject it.'} Track it in <b>Polls</b> — it applies as a new plan version only on approval.
      </p>
      <div className="compose-foot"><span className="spacer" /><button className="btn solid" onClick={onClose}>Done</button></div>
    </div>
  );
}

function ProposeChange({ stop, detail, days, tripId, onClose }: { stop: Stop; detail: PlanDetail; days: Day[]; tripId: string; onClose: () => void }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const place = detail.places.find((p) => p.id === stop.placeId);
  const ordered = [...days].sort((a, b) => a.date.localeCompare(b.date));
  const currentIndex = ordered.findIndex((d) => d.id === stop.dayId);
  const [toDayId, setToDayId] = useState(stop.dayId);
  const [why, setWhy] = useState('');
  const [route, setRoute] = useState<ProposalRoute>('leader_approval');
  const [sent, setSent] = useState(false);

  const toDay = ordered.find((d) => d.id === toDayId)!;
  const toIndex = ordered.indexOf(toDay);
  const moved = toDayId !== stop.dayId;
  const seq = detail.stops.filter((s) => s.dayId === toDayId).length + 1;
  const ops: ChangeOp[] = moved ? [{ op: 'move_stop', stopId: stop.id, toDayId, seq }] : [];

  const submit = useMutation({
    mutationFn: () =>
      api.createProposal(tripId, {
        title: `Move ${place?.name ?? 'stop'} to Day ${toIndex + 1}`,
        rationale: why.trim() || `Move ${place?.name ?? 'this stop'} from Day ${currentIndex + 1} to Day ${toIndex + 1}.`,
        changeSet: { basePlanVersion: detail.plan.version, ops },
        route,
      }),
    onSuccess: () => { queryClient.invalidateQueries(); setSent(true); },
  });

  if (sent) return <Sent route={route} onClose={onClose} />;

  return (
    <div className="compose">
      <div className="compose-head"><span className="kd" style={{ background: KIND_COLOR[stop.stopKind] }} /><strong>Propose a change · {place?.name}</strong><span className="badge">Day {currentIndex + 1}</span></div>
      <div className="field">
        <span className="fl">Move to day</span>
        <span className="fv">
          <select className="inp grow" value={toDayId} onChange={(e) => setToDayId(e.target.value)}>
            {ordered.map((d, i) => (
              <option key={d.id} value={d.id}>{dayOptionLabel(d, i)}</option>
            ))}
          </select>
        </span>
      </div>
      <div className="field"><span className="fl">Planned arrival</span><span className="fv"><span className="inp was">{stop.plannedArrival}</span><span className="hint">time stays a content edit — set it after the move applies</span></span></div>
      <div className="field" style={{ alignItems: 'start' }}><span className="fl">Why</span><span className="fv"><textarea className="inp grow" rows={2} placeholder="Sunset kills the grove's light by 16:45 — earlier + on Day 5 fixes it." value={why} onChange={(e) => setWhy(e.target.value)} /></span></div>

      {moved ? (
        <div className="preview">
          <span className="block-h">Preview · what leaders will see</span>
          <ChangeList ops={ops} detail={detail} />
        </div>
      ) : (
        <div className="warn">⚠ <span>Pick a different day — a move to the same day isn't a change.</span></div>
      )}

      <RouteSeg value={route} onChange={setRoute} />
      <div className="compose-foot">
        <span className="consequence">Moving a stop between days is <b>structural</b> — it goes to a leader (or a poll) and applies only on approval as a new plan version. <b>You won't see it live until then.</b></span>
        <button className="btn solid" disabled={!moved || submit.isPending} onClick={() => submit.mutate()}>Send to leaders →</button>
      </div>
    </div>
  );
}

/* ═══════════════ Propose a stop ═══════════════ */

function ProposeStop({ day, detail, candidates, tripId, onClose }: { day: Day; detail: PlanDetail; candidates: CandidateWithPlace[]; tripId: string; onClose: () => void }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const dayIndex = [...detail.days].sort((a, b) => a.date.localeCompare(b.date)).findIndex((d) => d.id === day.id);
  const shortlisted = candidates.filter((c) => c.status === 'shortlisted');
  const dayStops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === day.id);

  const [candidateId, setCandidateId] = useState(shortlisted[0]?.id ?? '');
  const [why, setWhy] = useState('');
  const [route, setRoute] = useState<ProposalRoute>('leader_approval');
  const [sent, setSent] = useState(false);

  const chosen = shortlisted.find((c) => c.id === candidateId);
  const ops: ChangeOp[] = chosen
    ? [{ op: 'add_stop', dayId: day.id, placeId: chosen.placeId, seq: dayStops.length + 1, stopKind: PLACE_TO_STOP_KIND[chosen.place.kind] }]
    : [];

  const submit = useMutation({
    mutationFn: () =>
      api.createProposal(tripId, {
        title: `Add ${chosen?.place.name ?? 'a stop'} to Day ${dayIndex + 1}`,
        rationale: why.trim() || chosen?.pitch || `Add ${chosen?.place.name ?? 'a stop'} to Day ${dayIndex + 1}.`,
        changeSet: { basePlanVersion: detail.plan.version, ops },
        route,
      }),
    onSuccess: () => { queryClient.invalidateQueries(); setSent(true); },
  });

  if (sent) return <Sent route={route} onClose={onClose} />;

  return (
    <div className="compose">
      <div className="compose-head"><span className="kd" style={{ background: KIND_COLOR.meal }} /><strong>Propose a stop · Day {dayIndex + 1} ({day.cityHint})</strong></div>
      <div className="field" style={{ alignItems: 'start' }}>
        <span className="fl">Start from</span>
        <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
          <div className="cand-pick">
            {shortlisted.length === 0 && <span className="muted">No candidates shortlisted yet — add one on the Candidates tab.</span>}
            {shortlisted.map((c) => (
              <button
                key={c.id}
                type="button"
                className={`cand-opt${c.id === candidateId ? ' sel' : ''}`}
                style={{ '--kc': PLACE_KIND_COLOR[c.place.kind] } as React.CSSProperties}
                onClick={() => setCandidateId(c.id)}
              >
                <span className="rg" />{c.place.name}
              </button>
            ))}
          </div>
        </span>
      </div>
      <div className="field"><span className="fl">Insert</span><span className="fv"><span className="inp">after {dayStops[dayStops.length - 1] ? detail.places.find((p) => p.id === dayStops[dayStops.length - 1].placeId)?.name : 'the start'}</span></span></div>
      <div className="field" style={{ alignItems: 'start' }}><span className="fl">Why</span><span className="fv"><textarea className="inp grow" rows={2} placeholder={chosen?.pitch ?? 'Why this place fits the day…'} value={why} onChange={(e) => setWhy(e.target.value)} /></span></div>

      {chosen && (
        <div className="preview">
          <span className="block-h">Preview</span>
          <ChangeList ops={ops} detail={detail} extraPlaces={[chosen.place]} />
          {feasibility && feasibility.feasibility !== 'ok' && (
            <div className="warn">⚠ <span>Day {dayIndex + 1} is already <b>{feasibility.feasibility} ({Math.round((feasibility.usedMin / feasibility.windowMin) * 100)}%)</b> — adding a stop will likely push it further. Leaders see this flag before deciding.</span></div>
          )}
        </div>
      )}

      <RouteSeg value={route} onChange={setRoute} />
      <div className="compose-foot">
        <span className="consequence">Adding a stop is <b>structural</b>. Submitting sends it to the leaders with the feasibility flag attached.</span>
        <button className="btn solid" disabled={!chosen || submit.isPending} onClick={() => submit.mutate()}>Send to leaders →</button>
      </div>
    </div>
  );
}
