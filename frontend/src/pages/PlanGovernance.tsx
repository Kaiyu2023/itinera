import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { useIsDesktop } from '../components/hooks';
import { KIND_COLOR, PLACE_KIND_COLOR } from './planShared';
import { ChangeList, PLACE_KIND_LABEL, PLACE_TO_STOP_KIND, dayOptionLabel, seqForSlot, slotOptions } from './governanceShared';
import type { CandidateWithPlace, ChangeOp, Day, NewPlaceDraft, PlaceKind, PlanDetail, ProposalRoute, Stop, Thread, User } from '../api/types';

/**
 * Wiring for the three Plan-tab stop actions (§ mockup d). A small context lets
 * the deeply-nested popover / sheet / panel buttons open one of three surfaces:
 * the Discuss thread, the Propose-change composer, or the Propose-a-stop
 * composer. Surfaces render as a centered modal (desktop) or a bottom sheet
 * (mobile) via <GovModalHost> — except the add-stop composer on the desktop map
 * view, which the map shell docks into its side panel instead (see PlanMap).
 */

export type GovAction =
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

/** State + setters for the governance surfaces, owned by the map views so they
    can both feed the modal host and (desktop map) dock the add-stop composer. */
export function usePlanActionsState(): { action: GovAction | null; actions: PlanActions; close: () => void } {
  const [action, setAction] = useState<GovAction | null>(null);
  const actions = useMemo<PlanActions>(
    () => ({
      discuss: (stop) => setAction({ kind: 'discuss', stop }),
      proposeChange: (stop) => setAction({ kind: 'change', stop }),
      proposeStop: (day) => setAction({ kind: 'addStop', day }),
    }),
    [],
  );
  return { action, actions, close: () => setAction(null) };
}

/** Provides the action setters to nested buttons (Discuss / Propose change / +). */
export function PlanActionsProvider({ actions, children }: { actions: PlanActions; children: ReactNode }) {
  return <PlanActionsContext.Provider value={actions}>{children}</PlanActionsContext.Provider>;
}

export interface GovData {
  tripId: string;
  detail: PlanDetail;
  days: Day[];
  candidates: CandidateWithPlace[];
  membersById: Map<string, User>;
  threads: Thread[];
}

/**
 * Renders the open governance surface as a modal/sheet. `dockAddStop` lets the
 * desktop map view claim the add-stop composer for its side panel — this host
 * then skips it so it isn't drawn twice.
 */
export function GovModalHost({ action, close, dockAddStop, ...data }: GovData & { action: GovAction | null; close: () => void; dockAddStop?: boolean }) {
  if (!action) return null;
  if (action.kind === 'addStop' && dockAddStop) return null;
  return (
    <GovModal onClose={close}>
      {action.kind === 'discuss' && (
        <ThreadPanel stop={action.stop} detail={data.detail} threads={data.threads} membersById={data.membersById} onClose={close} />
      )}
      {action.kind === 'change' && <ProposeChange stop={action.stop} detail={data.detail} days={data.days} tripId={data.tripId} onClose={close} />}
      {action.kind === 'addStop' && (
        <ProposeStopComposer day={action.day} detail={data.detail} candidates={data.candidates} tripId={data.tripId} onClose={close} />
      )}
    </GovModal>
  );
}

/* ═══════════════ modal chrome ═══════════════ */

function GovModal({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  const isDesktop = useIsDesktop();
  // Escape closes the topmost surface. A photo lightbox stacks above the modal,
  // so it owns Escape while up; the modal claims it otherwise.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.lb-backdrop')) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
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

/* ═══════════════ shared composer bits ═══════════════ */

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

/** Header ✕ shared by every composer — closes the surface. */
function ComposeClose({ onClose }: { onClose: () => void }) {
  return <button type="button" className="compose-x" onClick={onClose} aria-label="Close">✕</button>;
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

/* ═══════════════ Propose a change (Move | Remove) ═══════════════ */

type ChangeMode = 'move' | 'remove';

export function ProposeChange({ stop, detail, days, tripId, onClose }: { stop: Stop; detail: PlanDetail; days: Day[]; tripId: string; onClose: () => void }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const place = detail.places.find((p) => p.id === stop.placeId);
  const placeName = (id: string) => detail.places.find((p) => p.id === id)?.name ?? id;
  const ordered = [...days].sort((a, b) => a.date.localeCompare(b.date));
  const currentIndex = ordered.findIndex((d) => d.id === stop.dayId);

  const [mode, setMode] = useState<ChangeMode>('move');
  const [toDayId, setToDayId] = useState(stop.dayId);
  const [slot, setSlot] = useState<string>('');
  const [why, setWhy] = useState('');
  const [route, setRoute] = useState<ProposalRoute>('leader_approval');
  const [sent, setSent] = useState(false);

  const toIndex = ordered.findIndex((d) => d.id === toDayId);
  // Target-day slot options exclude the stop itself so you can't drop it after
  // where it already sits. Default: end of the target day.
  const targetStops = detail.stops.filter((s) => s.dayId === toDayId && s.id !== stop.id).sort((a, b) => a.seq - b.seq);
  const slotChoices = slotOptions(targetStops, placeName);
  const effectiveSlot = slot || slotChoices[slotChoices.length - 1].value;
  const seq = seqForSlot(effectiveSlot, targetStops);

  // The stop currently sitting just before this one — moving here is a no-op.
  const sameDayStops = detail.stops.filter((s) => s.dayId === stop.dayId).sort((a, b) => a.seq - b.seq);
  const selfIdx = sameDayStops.findIndex((s) => s.id === stop.id);
  const currentSlot = selfIdx <= 0 ? 'first' : sameDayStops[selfIdx - 1].id;
  const moved = mode === 'move' && (toDayId !== stop.dayId || effectiveSlot !== currentSlot);

  const ops: ChangeOp[] =
    mode === 'remove'
      ? [{ op: 'remove_stop', stopId: stop.id }]
      : moved
        ? [{ op: 'move_stop', stopId: stop.id, toDayId, seq }]
        : [];

  const canSubmit = mode === 'remove' ? why.trim().length > 0 : moved;

  const submit = useMutation({
    mutationFn: () =>
      api.createProposal(tripId, {
        title:
          mode === 'remove'
            ? `Remove ${place?.name ?? 'stop'} from Day ${currentIndex + 1}`
            : `Move ${place?.name ?? 'stop'} to Day ${toIndex + 1}`,
        rationale:
          why.trim() ||
          (mode === 'remove'
            ? `Drop ${place?.name ?? 'this stop'} from Day ${currentIndex + 1}.`
            : `Move ${place?.name ?? 'this stop'} from Day ${currentIndex + 1} to Day ${toIndex + 1}.`),
        changeSet: { basePlanVersion: detail.plan.version, ops },
        route,
      }),
    onSuccess: () => { queryClient.invalidateQueries(); setSent(true); },
  });

  if (sent) return <Sent route={route} onClose={onClose} />;

  return (
    <div className="compose">
      <div className="compose-head">
        <span className="kd" style={{ background: KIND_COLOR[stop.stopKind] }} />
        <strong>Propose a change · {place?.name}</strong>
        <span className="badge">Day {currentIndex + 1}</span>
        <ComposeClose onClose={onClose} />
      </div>

      <div className="field">
        <span className="fl">Action</span>
        <span className="fv">
          <span className="route-seg">
            <button type="button" className={mode === 'move' ? 'active' : ''} onClick={() => setMode('move')}>Move</button>
            <button type="button" className={mode === 'remove' ? 'active' : ''} onClick={() => setMode('remove')}>Remove</button>
          </span>
        </span>
      </div>

      {mode === 'move' ? (
        <>
          <div className="field">
            <span className="fl">Move to day</span>
            <span className="fv">
              <select className="inp grow" value={toDayId} onChange={(e) => { setToDayId(e.target.value); setSlot(''); }}>
                {ordered.map((d, i) => (
                  <option key={d.id} value={d.id}>{dayOptionLabel(d, i)}</option>
                ))}
              </select>
            </span>
          </div>
          <div className="field">
            <span className="fl">Position</span>
            <span className="fv">
              <select className="inp grow" value={effectiveSlot} onChange={(e) => setSlot(e.target.value)}>
                {slotChoices.map((o) => (
                  <option key={o.value} value={o.value}>{o.label}</option>
                ))}
              </select>
            </span>
          </div>
          <div className="field"><span className="fl">Planned arrival</span><span className="fv"><span className="inp was">{stop.plannedArrival}</span><span className="hint">time stays a content edit — set it after the move applies</span></span></div>
        </>
      ) : (
        <div className="field"><span className="fl">Dropping</span><span className="fv"><span className="inp was">{place?.name}</span><span className="hint">removes the stop from Day {currentIndex + 1}</span></span></div>
      )}

      <div className="field" style={{ alignItems: 'start' }}>
        <span className="fl">Why{mode === 'remove' ? ' *' : ''}</span>
        <span className="fv">
          <textarea
            className="inp grow"
            rows={2}
            placeholder={mode === 'remove' ? 'What frees up by dropping this stop?' : "Sunset kills the grove's light by 16:45 — earlier + on Day 5 fixes it."}
            value={why}
            onChange={(e) => setWhy(e.target.value)}
          />
        </span>
      </div>

      {ops.length > 0 ? (
        <div className="preview">
          <span className="block-h">Preview · what leaders will see</span>
          <ChangeList ops={ops} detail={detail} />
        </div>
      ) : mode === 'move' ? (
        <div className="warn">⚠ <span>Pick a different day or position — this move lands the stop right where it already is.</span></div>
      ) : (
        <div className="warn">⚠ <span>Removing a stop needs a reason — say what it frees up.</span></div>
      )}

      <RouteSeg value={route} onChange={setRoute} />
      <div className="compose-foot">
        <span className="consequence">{mode === 'remove' ? 'Removing' : 'Moving'} a stop is <b>structural</b> — it goes to a leader (or a poll) and applies only on approval as a new plan version. <b>You won't see it live until then.</b></span>
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="button" className="btn solid" disabled={!canSubmit || submit.isPending} onClick={() => submit.mutate()}>Send to leaders →</button>
      </div>
    </div>
  );
}

/* ═══════════════ Propose a stop (candidates | somewhere new) ═══════════════ */

type StopMode = 'candidates' | 'new';

/**
 * The add-stop composer. Two modes: pick a shortlisted candidate, or draft a
 * brand-new place. When docked into the desktop map panel the shell drives the
 * candidate selection (for the marker interplay) via the controlled props;
 * everywhere else the composer owns that state itself.
 */
export function ProposeStopComposer({
  day,
  detail,
  candidates,
  tripId,
  onClose,
  docked,
  mode: modeProp,
  onModeChange,
  candidateId: candidateIdProp,
  onCandidateChange,
}: {
  day: Day;
  detail: PlanDetail;
  candidates: CandidateWithPlace[];
  tripId: string;
  onClose: () => void;
  /** Rendered in the map side panel — swaps the panel content, keeps the map. */
  docked?: boolean;
  mode?: StopMode;
  onModeChange?: (m: StopMode) => void;
  candidateId?: string;
  onCandidateChange?: (id: string) => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const placeName = (id: string) => detail.places.find((p) => p.id === id)?.name ?? id;
  const dayIndex = [...detail.days].sort((a, b) => a.date.localeCompare(b.date)).findIndex((d) => d.id === day.id);
  const shortlisted = candidates.filter((c) => c.status === 'shortlisted');
  const dayStops = detail.stops.filter((s) => s.dayId === day.id).sort((a, b) => a.seq - b.seq);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === day.id);
  const cities = [...new Set([...detail.days].map((d) => d.cityHint))];

  // Candidate + mode may be controlled (docked) or internal (modal/sheet).
  const [modeI, setModeI] = useState<StopMode>('candidates');
  const [candidateIdI, setCandidateIdI] = useState(shortlisted[0]?.id ?? '');
  const mode = modeProp ?? modeI;
  const setMode = onModeChange ?? setModeI;
  const candidateId = candidateIdProp ?? candidateIdI;
  const setCandidateId = onCandidateChange ?? setCandidateIdI;

  // New-place draft + insert slot are always local to the composer.
  const [slot, setSlot] = useState<string>('');
  const [why, setWhy] = useState('');
  const [name, setName] = useState('');
  const [kind, setKind] = useState<PlaceKind>('sight');
  const [city, setCity] = useState(day.cityHint);
  const [note, setNote] = useState('');
  const [url, setUrl] = useState('');
  const [route, setRoute] = useState<ProposalRoute>('leader_approval');
  const [sent, setSent] = useState(false);

  const slotChoices = slotOptions(dayStops, placeName);
  const effectiveSlot = slot || slotChoices[slotChoices.length - 1].value;
  const seq = seqForSlot(effectiveSlot, dayStops);

  const chosen = shortlisted.find((c) => c.id === candidateId);
  const trimmedName = name.trim();
  const newDraft: NewPlaceDraft = { name: trimmedName, kind, city, note: note.trim(), url: url.trim() || null };

  const ops: ChangeOp[] =
    mode === 'new'
      ? trimmedName
        ? [{ op: 'add_place_stop', dayId: day.id, seq, stopKind: PLACE_TO_STOP_KIND[kind], draft: newDraft }]
        : []
      : chosen
        ? [{ op: 'add_stop', dayId: day.id, placeId: chosen.placeId, seq, stopKind: PLACE_TO_STOP_KIND[chosen.place.kind] }]
        : [];

  const canSubmit = mode === 'new' ? trimmedName.length > 0 : !!chosen;
  const addedName = mode === 'new' ? trimmedName || 'a place' : chosen?.place.name ?? 'a stop';

  const submit = useMutation({
    mutationFn: () =>
      api.createProposal(tripId, {
        title: `Add ${addedName} to Day ${dayIndex + 1}`,
        rationale: why.trim() || (mode === 'candidates' ? chosen?.pitch : '') || `Add ${addedName} to Day ${dayIndex + 1}.`,
        changeSet: { basePlanVersion: detail.plan.version, ops },
        route,
      }),
    onSuccess: () => { queryClient.invalidateQueries(); setSent(true); },
  });

  if (sent) return <Sent route={route} onClose={onClose} />;

  return (
    <div className={`compose${docked ? ' compose-docked' : ''}`}>
      <div className="compose-head">
        <span className="kd" style={{ background: KIND_COLOR.meal }} />
        <strong>Propose a stop · Day {dayIndex + 1} ({day.cityHint})</strong>
        <ComposeClose onClose={onClose} />
      </div>

      <div className="field">
        <span className="fl">Add</span>
        <span className="fv">
          <span className="route-seg">
            <button type="button" className={mode === 'candidates' ? 'active' : ''} onClick={() => setMode('candidates')}>From candidates</button>
            <button type="button" className={mode === 'new' ? 'active' : ''} onClick={() => setMode('new')}>Somewhere new</button>
          </span>
        </span>
      </div>

      {mode === 'candidates' ? (
        <div className="field" style={{ alignItems: 'start' }}>
          <span className="fl">Start from</span>
          <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
            <div className="cand-pick">
              {shortlisted.length === 0 && <span className="muted">No candidates shortlisted yet — add one on the Candidates tab, or switch to “Somewhere new”.</span>}
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
            {docked && shortlisted.length > 0 && <span className="hint">Tip: click a candidate ring on the map to pick it here.</span>}
          </span>
        </div>
      ) : (
        <>
          <div className="field"><span className="fl">Name *</span><span className="fv"><input className="inp grow" placeholder="e.g. Kissa Master (kissaten)" value={name} onChange={(e) => setName(e.target.value)} /></span></div>
          <div className="field">
            <span className="fl">Kind</span>
            <span className="fv">
              <select className="inp grow" value={kind} onChange={(e) => setKind(e.target.value as PlaceKind)}>
                {(Object.keys(PLACE_KIND_LABEL) as PlaceKind[]).map((k) => (
                  <option key={k} value={k}>{PLACE_KIND_LABEL[k]}</option>
                ))}
              </select>
            </span>
          </div>
          <div className="field">
            <span className="fl">City</span>
            <span className="fv">
              <select className="inp grow" value={city} onChange={(e) => setCity(e.target.value)}>
                {cities.map((c) => (
                  <option key={c} value={c}>{c}</option>
                ))}
              </select>
            </span>
          </div>
          <div className="field"><span className="fl">Link</span><span className="fv"><input className="inp grow" placeholder="Google Maps or website (optional)" value={url} onChange={(e) => setUrl(e.target.value)} /></span></div>
          <div className="field" style={{ alignItems: 'start' }}><span className="fl">Note</span><span className="fv"><textarea className="inp grow" rows={2} placeholder="Anything the group should know (optional)" value={note} onChange={(e) => setNote(e.target.value)} /></span></div>
        </>
      )}

      <div className="field">
        <span className="fl">Insert</span>
        <span className="fv">
          <select className="inp grow" value={effectiveSlot} onChange={(e) => setSlot(e.target.value)}>
            {slotChoices.map((o) => (
              <option key={o.value} value={o.value}>{o.label}</option>
            ))}
          </select>
        </span>
      </div>

      <div className="field" style={{ alignItems: 'start' }}>
        <span className="fl">Why</span>
        <span className="fv"><textarea className="inp grow" rows={2} placeholder={(mode === 'candidates' ? chosen?.pitch : '') || 'Why this place fits the day…'} value={why} onChange={(e) => setWhy(e.target.value)} /></span>
      </div>

      {ops.length > 0 && (
        <div className="preview">
          <span className="block-h">Preview</span>
          <ChangeList ops={ops} detail={detail} extraPlaces={chosen ? [chosen.place] : []} />
          {feasibility && feasibility.feasibility !== 'ok' && (
            <div className="warn">⚠ <span>Day {dayIndex + 1} is already <b>{feasibility.feasibility} ({Math.round((feasibility.usedMin / feasibility.windowMin) * 100)}%)</b> — adding a stop will likely push it further. Leaders see this flag before deciding.</span></div>
          )}
        </div>
      )}

      <RouteSeg value={route} onChange={setRoute} />
      <div className="compose-foot">
        <span className="consequence">Adding a stop is <b>structural</b>. Submitting sends it to the leaders with the feasibility flag attached.</span>
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="button" className="btn solid" disabled={!canSubmit || submit.isPending} onClick={() => submit.mutate()}>Send to leaders →</button>
      </div>
    </div>
  );
}
