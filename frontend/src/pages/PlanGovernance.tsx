import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useSearchParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { useIsDesktop } from '../components/hooks';
import { MapView } from '../map/MapView';
import type { LngLat } from '../map/MapRenderer';
import { KIND_COLOR, PLACE_KIND_COLOR } from './planShared';
import {
  ChangeList,
  PLACE_KIND_LABEL,
  PLACE_TO_STOP_KIND,
  dayOptionLabel,
  projectFeasibilityAfterAdd,
  seqForSlot,
  slotOptions,
} from './governanceShared';
import {
  EMBED_PAD,
  buildDayGeo,
  dayMarkers,
  dayRoutes,
  padBounds,
  proposedDayRoutes,
  proposedStopMarker,
  searchResultMarkers,
} from './planMapGeometry';
import type {
  CandidateWithPlace,
  ChangeOp,
  Day,
  NewPlaceDraft,
  Place,
  PlaceKind,
  PlanDetail,
  ProposalRoute,
  Stop,
  Thread,
  User,
} from '../api/types';

/**
 * Wiring for the three Plan-tab stop actions (§ mockup d). A small context lets
 * the deeply-nested popover / sheet / panel buttons open one of three surfaces:
 * the Discuss thread, the Propose-change composer, or the Propose-a-stop
 * composer. Surfaces render as a centered modal (desktop) or a bottom sheet
 * (mobile) via <GovModalHost> — except the add-stop composer on the desktop map
 * view, which the map shell docks into its side panel instead (see PlanMap).
 */

export type GovAction =
  { kind: 'discuss'; stop: Stop } | { kind: 'change'; stop: Stop } | { kind: 'addStop'; day: Day };

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

/** The open governance surface + its setters. Hoisted to PlanTab so a single
    host owns the modals across the timeline and both map views. */
export interface GovState {
  action: GovAction | null;
  actions: PlanActions;
  close: () => void;
}

/** State + setters for the governance surfaces. PlanTab owns one of these and
    threads it down to the map shell (which docks the add-stop composer). */
export function usePlanActionsState(): GovState {
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

/* ═══════════════ place search (shared by the composer + docked map) ═══════════════ */

export interface StopSearchController {
  query: string;
  setQuery: (q: string) => void;
  results: Place[];
  loading: boolean;
  selectedId: string | null;
  select: (id: string | null) => void;
  selected: Place | null;
  /** Arm the next results batch to auto-select its first hit (deep links). */
  pickFirstOnNext: () => void;
  clear: () => void;
}

/**
 * Debounced place search over the ApiClient's `searchPlaces` port. Owns the
 * query, the results, and which result is picked. Lives wherever the search
 * pins need to render: the composer owns one for its embedded map; the desktop
 * map shell owns one so the hits become pins on the live map.
 */
export function useStopSearch(): StopSearchController {
  const api = useApi();
  const [query, setQueryState] = useState('');
  const [results, setResults] = useState<Place[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const pickFirstRef = useRef(false);
  const reqRef = useRef(0);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    const myReq = ++reqRef.current;
    const t = setTimeout(() => {
      api
        .searchPlaces(q)
        .then((r) => {
          if (reqRef.current !== myReq) return; // a newer query superseded this one
          setResults(r);
          setLoading(false);
          if (pickFirstRef.current) {
            pickFirstRef.current = false;
            setSelectedId(r[0]?.id ?? null);
          }
        })
        .catch(() => {
          if (reqRef.current !== myReq) return;
          setResults([]);
          setLoading(false);
        });
    }, 250);
    return () => clearTimeout(t);
  }, [query, api]);

  const selected = results.find((r) => r.id === selectedId) ?? null;
  return {
    query,
    setQuery: setQueryState,
    results,
    loading,
    selectedId,
    select: setSelectedId,
    selected,
    pickFirstOnNext: () => {
      pickFirstRef.current = true;
    },
    clear: () => {
      reqRef.current++;
      pickFirstRef.current = false;
      setQueryState('');
      setResults([]);
      setSelectedId(null);
      setLoading(false);
    },
  };
}

/** Parse the add-stop deep link (`?gov=addStop&mode=&q=&pick=`) for a given
    day; null when it isn't addressed to this composer. A genuine deep-link
    feature that also drives the review screenshots. */
export function readAddStopDeepLink(
  params: URLSearchParams,
  dayId: string,
): { mode: StopMode | null; query: string | null; pickFirst: boolean; candidate: string | null } | null {
  if (params.get('gov') !== 'addStop') return null;
  const dayParam = params.get('day');
  if (dayParam && dayParam !== dayId) return null;
  const m = params.get('mode');
  return {
    mode: m === 'new' || m === 'candidates' ? m : null,
    query: params.get('q'),
    pickFirst: params.get('pick') === 'first',
    candidate: params.get('candidate'),
  };
}

/** Drop the one-shot add-stop deep-link params so a later manual open is clean. */
export function stripAddStopDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  ['gov', 'mode', 'q', 'pick', 'candidate', 'day'].forEach((k) => next.delete(k));
  return next;
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
export function GovModalHost({
  action,
  close,
  dockAddStop,
  ...data
}: GovData & { action: GovAction | null; close: () => void; dockAddStop?: boolean }) {
  if (!action) return null;
  if (action.kind === 'addStop' && dockAddStop) return null;
  return (
    <GovModal onClose={close} wide={action.kind === 'addStop'}>
      {(requestClose) => (
        <>
          {action.kind === 'discuss' && (
            <ThreadPanel
              stop={action.stop}
              detail={data.detail}
              threads={data.threads}
              tripId={data.tripId}
              membersById={data.membersById}
              onClose={requestClose}
            />
          )}
          {action.kind === 'change' && (
            <ProposeChange
              stop={action.stop}
              detail={data.detail}
              days={data.days}
              tripId={data.tripId}
              onClose={requestClose}
            />
          )}
          {action.kind === 'addStop' && (
            <ProposeStopComposer
              day={action.day}
              detail={data.detail}
              days={data.days}
              candidates={data.candidates}
              tripId={data.tripId}
              onClose={requestClose}
            />
          )}
        </>
      )}
    </GovModal>
  );
}

/* ═══════════════ modal chrome ═══════════════ */

function GovModal({
  children,
  onClose,
  wide,
}: {
  children: (requestClose: () => void) => ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  const isDesktop = useIsDesktop();
  // Close is orchestrated: flag `closing` to swap in the exit animation, then
  // fire `onClose` when the backdrop's own animation ends. `requestClose` is
  // handed to the composers so their Cancel / ✕ / Done buttons animate out too.
  const [closing, setClosing] = useState(false);
  const requestClose = useCallback(() => setClosing(true), []);
  // Escape closes the topmost surface. A photo lightbox stacks above the modal,
  // so it owns Escape while up; the modal claims it otherwise.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.lb-backdrop')) requestClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [requestClose]);
  return (
    <div
      className={`gov-backdrop${closing ? ' closing' : ''}`}
      onClick={requestClose}
      onAnimationEnd={(e) => {
        if (closing && e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className={`gov-modal${isDesktop ? '' : ' sheet'}${wide ? ' wide' : ''}`}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        {!isDesktop && (
          <div className="gov-grip">
            <span />
          </div>
        )}
        {children(requestClose)}
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
  tripId,
  membersById,
  onClose,
}: {
  stop: Stop;
  detail: PlanDetail;
  threads: Thread[];
  tripId: string;
  membersById: Map<string, User>;
  onClose: () => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const place = detail.places.find((p) => p.id === stop.placeId);
  const dayIndex = [...detail.days].sort((a, b) => a.date.localeCompare(b.date)).findIndex((d) => d.id === stop.dayId);
  // A freshly-started thread shows live before the parent's threads query refetches.
  const [localThread, setLocalThread] = useState<Thread | null>(null);
  const thread = threads.find((t) => t.anchor.kind === 'stop' && t.anchor.stopId === stop.id) ?? localThread;
  const [draft, setDraft] = useState('');
  const [startDraft, setStartDraft] = useState('');

  const start = useMutation({
    mutationFn: (body: string) =>
      api.createThread(tripId, { anchor: { kind: 'stop', stopId: stop.id }, title: place?.name ?? 'Discussion', body }),
    onSuccess: (t) => {
      setStartDraft('');
      setLocalThread(t);
      queryClient.invalidateQueries({ queryKey: ['threads'] });
    },
  });

  const comments = useQuery({
    queryKey: ['comments', thread?.id],
    queryFn: () => api.getComments(thread!.id),
    enabled: !!thread,
  });
  const post = useMutation({
    mutationFn: (body: string) => api.addComment(thread!.id, body),
    onSuccess: () => {
      setDraft('');
      queryClient.invalidateQueries({ queryKey: ['comments', thread?.id] });
      queryClient.invalidateQueries({ queryKey: ['threads'] });
    },
  });
  const react = useMutation({
    mutationFn: ({ commentId, emoji }: { commentId: string; emoji: string }) => api.toggleReaction(commentId, emoji),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['comments', thread?.id] }),
  });

  return (
    <div className="panel-card">
      <div className="panel-top">
        <span className="anchor">
          <span className="kd" style={{ background: KIND_COLOR[stop.stopKind] }} />
          {place?.name} · Day {dayIndex + 1}
        </span>
        <button type="button" className="close" onClick={onClose} aria-label="Close">
          ✕
        </button>
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
                  <span className="avatar sm" style={{ background: author?.avatarColor ?? '#888' }}>
                    {author?.displayName[0] ?? '?'}
                  </span>
                  <div>
                    <div className="bubble">
                      <div className="ch">
                        <span className="nm">{author?.displayName ?? '—'}</span>
                        <span className="tm">
                          {new Date(c.createdAt).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}
                        </span>
                      </div>
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
                      <button
                        type="button"
                        className="r add"
                        onClick={() => react.mutate({ commentId: c.id, emoji: '👍' })}
                      >
                        +👍
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <form
            className="composer"
            onSubmit={(e) => {
              e.preventDefault();
              if (draft.trim()) post.mutate(draft.trim());
            }}
          >
            <span className="avatar sm" style={{ background: me.data?.avatarColor ?? '#6b5bd2' }}>
              {me.data?.displayName[0] ?? 'K'}
            </span>
            <input
              className="in"
              placeholder="Add to the thread…"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
            <button className="btn solid sm" type="submit" disabled={!draft.trim() || post.isPending}>
              Send
            </button>
          </form>
        </>
      ) : (
        <>
          <div className="thread-body">
            <p className="muted">
              No discussion on this stop yet — kick one off. It threads under <b>{place?.name}</b>.
            </p>
          </div>
          <form
            className="composer start"
            onSubmit={(e) => {
              e.preventDefault();
              if (startDraft.trim()) start.mutate(startDraft.trim());
            }}
          >
            <span className="avatar sm" style={{ background: me.data?.avatarColor ?? '#6b5bd2' }}>
              {me.data?.displayName[0] ?? 'K'}
            </span>
            <textarea
              className="in"
              rows={2}
              placeholder="Start the discussion…"
              value={startDraft}
              onChange={(e) => setStartDraft(e.target.value)}
            />
            <button className="btn solid sm" type="submit" disabled={!startDraft.trim() || start.isPending}>
              Start
            </button>
          </form>
        </>
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
        <button
          type="button"
          className={value === 'leader_approval' ? 'active' : ''}
          onClick={() => onChange('leader_approval')}
        >
          Request a leader's approval
        </button>
        <button type="button" className={value === 'poll' ? 'active' : ''} onClick={() => onChange('poll')}>
          Open a poll
        </button>
      </span>
    </div>
  );
}

/** Header ✕ shared by every composer — closes the surface. */
function ComposeClose({ onClose }: { onClose: () => void }) {
  return (
    <button type="button" className="compose-x" onClick={onClose} aria-label="Close">
      ✕
    </button>
  );
}

function Sent({ route, onClose }: { route: ProposalRoute; onClose: () => void }) {
  return (
    <div className="compose sent">
      <strong>{route === 'poll' ? 'Poll opened ✓' : 'Sent to leaders ✓'}</strong>
      <p className="muted">
        {route === 'poll' ? 'A poll is open for the group to decide.' : 'A leader will approve or reject it.'} Track it
        in <b>Polls</b> — it applies as a new plan version only on approval.
      </p>
      <div className="compose-foot">
        <span className="spacer" />
        <button className="btn solid" onClick={onClose}>
          Done
        </button>
      </div>
    </div>
  );
}

/* ═══════════════ Propose a change (Move | Remove) ═══════════════ */

type ChangeMode = 'move' | 'remove';

export function ProposeChange({
  stop,
  detail,
  days,
  tripId,
  onClose,
}: {
  stop: Stop;
  detail: PlanDetail;
  days: Day[];
  tripId: string;
  onClose: () => void;
}) {
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
  const [route, setRoute] = useState<ProposalRoute>('poll');
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
    onSuccess: () => {
      queryClient.invalidateQueries();
      setSent(true);
    },
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
            <button type="button" className={mode === 'move' ? 'active' : ''} onClick={() => setMode('move')}>
              Move
            </button>
            <button type="button" className={mode === 'remove' ? 'active' : ''} onClick={() => setMode('remove')}>
              Remove
            </button>
          </span>
        </span>
      </div>

      {mode === 'move' ? (
        <>
          <div className="field">
            <span className="fl">Move to day</span>
            <span className="fv">
              <select
                className="inp grow"
                value={toDayId}
                onChange={(e) => {
                  setToDayId(e.target.value);
                  setSlot('');
                }}
              >
                {ordered.map((d, i) => (
                  <option key={d.id} value={d.id}>
                    {dayOptionLabel(d, i)}
                  </option>
                ))}
              </select>
            </span>
          </div>
          <div className="field">
            <span className="fl">Position</span>
            <span className="fv">
              <select className="inp grow" value={effectiveSlot} onChange={(e) => setSlot(e.target.value)}>
                {slotChoices.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </span>
          </div>
          <div className="field">
            <span className="fl">Planned arrival</span>
            <span className="fv">
              <span className="inp was">{stop.plannedArrival}</span>
              <span className="hint">time stays a content edit — set it after the move applies</span>
            </span>
          </div>
        </>
      ) : (
        <div className="field">
          <span className="fl">Dropping</span>
          <span className="fv">
            <span className="inp was">{place?.name}</span>
            <span className="hint">removes the stop from Day {currentIndex + 1}</span>
          </span>
        </div>
      )}

      <div className="field" style={{ alignItems: 'start' }}>
        <span className="fl">Why{mode === 'remove' ? ' *' : ''}</span>
        <span className="fv">
          <textarea
            className="inp grow"
            rows={2}
            placeholder={
              mode === 'remove'
                ? 'What frees up by dropping this stop?'
                : "Sunset kills the grove's light by 16:45 — earlier + on Day 5 fixes it."
            }
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
        <div className="warn">
          ⚠ <span>Pick a different day or position — this move lands the stop right where it already is.</span>
        </div>
      ) : (
        <div className="warn">
          ⚠ <span>Removing a stop needs a reason — say what it frees up.</span>
        </div>
      )}

      <RouteSeg value={route} onChange={setRoute} />
      <div className="compose-foot">
        <span className="spacer" />
        <button type="button" className="btn" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn solid"
          disabled={!canSubmit || submit.isPending}
          onClick={() => submit.mutate()}
        >
          {route === 'poll' ? 'Open the poll →' : 'Send to leaders →'}
        </button>
      </div>
    </div>
  );
}

/* ═══════════════ Propose a stop (candidates | somewhere new) ═══════════════ */

type StopMode = 'candidates' | 'new';

/**
 * The add-stop composer. Two modes: pick a shortlisted candidate, or draft a
 * brand-new place found by searching the map.
 *
 * - **Docked** (desktop map view): the shell drives candidate + search state via
 *   the controlled props so the hits become live pins on the *main* map.
 * - **Modal / sheet** (timeline + mobile): the composer owns that state and
 *   embeds its own {@link MapView} — day context markers plus search-result pins,
 *   two-way selectable with the result list.
 *
 * Selecting a search hit prefills name/kind/city/coordinates (all still
 * editable); a hit that's already a trip place reuses it via `add_stop` instead
 * of minting a duplicate. Manual entry works when nothing is found.
 */
export function ProposeStopComposer({
  day,
  detail,
  days,
  candidates,
  tripId,
  onClose,
  docked,
  mode: modeProp,
  onModeChange,
  candidateId: candidateIdProp,
  onCandidateChange,
  search: searchProp,
  onPreviewChange,
}: {
  day: Day;
  detail: PlanDetail;
  days: Day[];
  candidates: CandidateWithPlace[];
  tripId: string;
  onClose: () => void;
  /** Rendered in the map side panel — swaps the panel content, keeps the map. */
  docked?: boolean;
  mode?: StopMode;
  onModeChange?: (m: StopMode) => void;
  candidateId?: string;
  onCandidateChange?: (id: string) => void;
  /** Supplied when docked so the search pins land on the shell's live map. */
  search?: StopSearchController;
  /** Docked only: report the insert-outcome preview (point + new seq) up to the
      shell so it can splice it onto the live map. Null when nothing to preview. */
  onPreviewChange?: (preview: { insertAt: LngLat; seq: number } | null) => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const [urlParams, setUrlParams] = useSearchParams();
  const placeName = (id: string) => detail.places.find((p) => p.id === id)?.name ?? id;
  const orderedDays = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const shortlisted = candidates.filter((c) => c.status === 'shortlisted');
  const cities = [...new Set([...detail.days].map((d) => d.cityHint))];

  // Opened without a fixed day (candidate → plan deep link): let the composer
  // pick the day itself. From a day's "＋ Propose a stop" (or a `day=` link) the
  // day is fixed and this select never appears.
  const [pickDay] = useState(() => {
    if (searchProp) return false; // the docked shell always opens on a fixed day
    const link = readAddStopDeepLink(urlParams, day.id);
    return !!link && !urlParams.get('day');
  });
  const [dayId, setDayId] = useState(day.id);
  const activeDay = orderedDays.find((d) => d.id === dayId) ?? day;
  const dayIndex = orderedDays.findIndex((d) => d.id === activeDay.id);
  const dayStops = detail.stops.filter((s) => s.dayId === activeDay.id).sort((a, b) => a.seq - b.seq);
  const feasibility = detail.dayFeasibility.find((f) => f.dayId === activeDay.id);

  // Candidate + mode may be controlled (docked) or internal (modal/sheet).
  const [modeI, setModeI] = useState<StopMode>('candidates');
  const [candidateIdI, setCandidateIdI] = useState(shortlisted[0]?.id ?? '');
  const mode = modeProp ?? modeI;
  const setMode = onModeChange ?? setModeI;
  const candidateId = candidateIdProp ?? candidateIdI;
  const setCandidateId = onCandidateChange ?? setCandidateIdI;

  // Search: the shell's controller when docked, otherwise our own. (The hook
  // still runs when a prop is supplied; with an empty query it does nothing.)
  const ownSearch = useStopSearch();
  const search = searchProp ?? ownSearch;

  // New-place draft + insert slot are always local to the composer.
  const [slot, setSlot] = useState<string>('');
  const [why, setWhy] = useState('');
  const [name, setName] = useState('');
  const [kind, setKind] = useState<PlaceKind>('sight');
  const [city, setCity] = useState(day.cityHint);
  const [note, setNote] = useState('');
  const [url, setUrl] = useState('');
  const [coord, setCoord] = useState<LngLat | null>(null); // set when a search hit is picked
  const [route, setRoute] = useState<ProposalRoute>('poll');
  const [sent, setSent] = useState(false);

  // Picking a search hit prefills the form; fields stay editable afterwards.
  const lastPrefilled = useRef<string | null>(null);
  useEffect(() => {
    const sel = search.selected;
    if (sel && sel.id !== lastPrefilled.current) {
      lastPrefilled.current = sel.id;
      setName(sel.name);
      setKind(sel.kind);
      setCity(sel.city);
      setCoord({ lng: sel.lng, lat: sel.lat });
    }
  }, [search.selected]);

  const clearSelection = () => {
    search.select(null);
    setCoord(null);
    lastPrefilled.current = null;
  };

  // One-shot add-stop deep link (?gov=addStop&mode=&q=&pick=). Only the composer
  // that owns its search consumes it; the docked shell handles its own. We strip
  // the params so a later manual open starts clean.
  const booted = useRef(false);
  useEffect(() => {
    if (searchProp || booted.current) return;
    booted.current = true;
    const link = readAddStopDeepLink(urlParams, day.id);
    if (!link) return;
    if (link.mode) setMode(link.mode);
    // A candidate deep link (from "Propose for the plan →") preselects it.
    if (link.candidate) {
      setMode('candidates');
      setCandidateId(link.candidate);
    }
    if (link.query) {
      search.setQuery(link.query);
      if (link.pickFirst) search.pickFirstOnNext();
    }
    setUrlParams(stripAddStopDeepLink(urlParams), { replace: true });
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const slotChoices = slotOptions(dayStops, placeName);
  const effectiveSlot = slot || slotChoices[slotChoices.length - 1].value;
  const seq = seqForSlot(effectiveSlot, dayStops);

  const chosen = shortlisted.find((c) => c.id === candidateId);
  const trimmedName = name.trim();
  const selectedResult = search.selected;
  // A hit that's already in the plan is re-added by reference, not re-minted.
  const selectedIsTripPlace = !!selectedResult && detail.places.some((p) => p.id === selectedResult.id);
  const newDraft: NewPlaceDraft = {
    name: trimmedName,
    kind,
    city,
    note: note.trim(),
    url: url.trim() || null,
    lat: coord?.lat ?? null,
    lng: coord?.lng ?? null,
  };

  const ops: ChangeOp[] =
    mode === 'new'
      ? selectedIsTripPlace && selectedResult
        ? [{ op: 'add_stop', dayId: activeDay.id, placeId: selectedResult.id, seq, stopKind: PLACE_TO_STOP_KIND[kind] }]
        : trimmedName
          ? [{ op: 'add_place_stop', dayId: activeDay.id, seq, stopKind: PLACE_TO_STOP_KIND[kind], draft: newDraft }]
          : []
      : chosen
        ? [
            {
              op: 'add_stop',
              dayId: activeDay.id,
              placeId: chosen.placeId,
              seq,
              stopKind: PLACE_TO_STOP_KIND[chosen.place.kind],
            },
          ]
        : [];

  const canSubmit = mode === 'new' ? trimmedName.length > 0 || selectedIsTripPlace : !!chosen;
  const addedName = mode === 'new' ? trimmedName || 'a place' : (chosen?.place.name ?? 'a stop');

  // Insert-outcome preview: where the picked place lands (a candidate's coords,
  // or a search-hit / map-pinned coord in "new" mode) and its resulting 1-based
  // stop number. `seq` is fractional (0.5 lands first); the integer index is how
  // many stops sit before it. A hand-entered place with no coordinates has
  // nothing to place, so there is no outcome to draw.
  const insertAt: LngLat | null =
    mode === 'candidates' ? (chosen ? { lng: chosen.place.lng, lat: chosen.place.lat } : null) : coord;
  const previewSeq = dayStops.filter((s) => s.seq < seq).length + 1;
  const previewAt = ops.length > 0 ? insertAt : null;

  const submit = useMutation({
    mutationFn: () =>
      api.createProposal(tripId, {
        title: `Add ${addedName} to Day ${dayIndex + 1}`,
        rationale:
          why.trim() || (mode === 'candidates' ? chosen?.pitch : '') || `Add ${addedName} to Day ${dayIndex + 1}.`,
        changeSet: { basePlanVersion: detail.plan.version, ops },
        route,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries();
      setSent(true);
    },
  });

  // The composer's own embedded map (modal / sheet only — docked uses the live
  // map). Day context markers, plus candidate rings or search-result pins.
  const dayGeo = useMemo(
    () => buildDayGeo(detail, days, activeDay, candidates, EMBED_PAD),
    [detail, days, activeDay, candidates],
  );
  const embedMarkers = useMemo(() => {
    const pick = mode === 'candidates' ? { interactive: true, selectedId: candidateId } : undefined;
    const base = dayMarkers(dayGeo, null, mode === 'candidates', pick);
    const withHits = mode === 'new' ? [...base, ...searchResultMarkers(search.results, search.selectedId)] : base;
    return previewAt ? [...withHits, proposedStopMarker(previewAt, previewSeq)] : withHits;
  }, [dayGeo, mode, candidateId, search.results, search.selectedId, previewAt?.lng, previewAt?.lat, previewSeq]);
  const embedBounds = useMemo(() => {
    const extra: LngLat[] = [];
    if (mode === 'new') extra.push(...search.results.map((r) => ({ lng: r.lng, lat: r.lat })));
    if (previewAt) extra.push(previewAt);
    if (!extra.length) return dayGeo.bounds;
    const dayPts = dayGeo.stops
      .map((s) => dayGeo.placeById.get(s.placeId))
      .filter((p): p is Place => !!p)
      .map((p) => ({ lng: p.lng, lat: p.lat }));
    if (dayGeo.home) dayPts.push({ lng: dayGeo.home.lng, lat: dayGeo.home.lat });
    return padBounds([...dayPts, ...extra], EMBED_PAD);
  }, [dayGeo, mode, search.results, previewAt?.lng, previewAt?.lat]);
  const embedRoutes = useMemo(
    () => (previewAt ? proposedDayRoutes(dayGeo, previewAt, previewSeq) : dayRoutes(dayGeo)),
    [dayGeo, previewAt?.lng, previewAt?.lat, previewSeq],
  );

  // Docked: hand the insert-outcome preview to the shell so it lands on the live
  // map. Fire on change, and clear on unmount so a closed composer leaves no pin.
  useEffect(() => {
    onPreviewChange?.(previewAt ? { insertAt: previewAt, seq: previewSeq } : null);
  }, [onPreviewChange, previewAt?.lng, previewAt?.lat, previewSeq]);
  useEffect(() => () => onPreviewChange?.(null), [onPreviewChange]);

  if (sent) return <Sent route={route} onClose={onClose} />;

  const searchBox = (
    <div className="field" style={{ alignItems: 'start' }}>
      <span className="fl">Search</span>
      <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
        <input
          className="inp grow"
          placeholder="Search places…"
          value={search.query}
          onChange={(e) => search.setQuery(e.target.value)}
        />
        {search.query.trim() && (
          <div className="place-results">
            {search.loading && <span className="muted pr-status">Searching…</span>}
            {!search.loading && search.results.length === 0 && (
              <span className="muted pr-status">No matches — fill in the details below to add it by hand.</span>
            )}
            {search.results.map((r) => (
              <button
                key={r.id}
                type="button"
                className={`place-result${r.id === search.selectedId ? ' sel' : ''}`}
                style={{ '--kc': PLACE_KIND_COLOR[r.kind] } as CSSProperties}
                onClick={() => search.select(r.id)}
              >
                <span className="pr-dot" />
                <span className="pr-main">
                  <span className="pr-name">{r.name}</span>
                  <span className="pr-sub">
                    {PLACE_KIND_LABEL[r.kind]} · {r.city}
                  </span>
                </span>
                {detail.places.some((p) => p.id === r.id) && <span className="badge">in trip</span>}
              </button>
            ))}
          </div>
        )}
        {selectedResult && (
          <button type="button" className="clear-sel" onClick={clearSelection}>
            ✕ Clear selection — enter by hand
          </button>
        )}
        {docked && <span className="hint">Hits drop as pins on the map — click one to pick it.</span>}
      </span>
    </div>
  );

  const fields = (
    <>
      {pickDay && (
        <div className="field">
          <span className="fl">Day</span>
          <span className="fv">
            <select
              className="inp grow"
              value={dayId}
              onChange={(e) => {
                setDayId(e.target.value);
                setSlot('');
              }}
            >
              {orderedDays.map((d, i) => (
                <option key={d.id} value={d.id}>
                  {dayOptionLabel(d, i)}
                </option>
              ))}
            </select>
          </span>
        </div>
      )}
      <div className="field">
        <span className="fl">Add</span>
        <span className="fv">
          <span className="route-seg">
            <button
              type="button"
              className={mode === 'candidates' ? 'active' : ''}
              onClick={() => setMode('candidates')}
            >
              From candidates
            </button>
            <button type="button" className={mode === 'new' ? 'active' : ''} onClick={() => setMode('new')}>
              Somewhere new
            </button>
          </span>
        </span>
      </div>

      {mode === 'candidates' ? (
        <div className="field" style={{ alignItems: 'start' }}>
          <span className="fl">Candidate</span>
          <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
            <div className="cand-pick">
              {shortlisted.length === 0 && (
                <span className="muted">
                  No candidates shortlisted yet — add one on the Candidates tab, or switch to “Somewhere new”.
                </span>
              )}
              {shortlisted.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  className={`cand-opt${c.id === candidateId ? ' sel' : ''}`}
                  style={{ '--kc': PLACE_KIND_COLOR[c.place.kind] } as CSSProperties}
                  onClick={() => setCandidateId(c.id)}
                >
                  <span className="rg" />
                  {c.place.name}
                </button>
              ))}
            </div>
            <span className="hint">Tip: click a candidate ring on the map to pick it here.</span>
          </span>
        </div>
      ) : (
        <>
          {searchBox}
          <div className="field">
            <span className="fl">Name *</span>
            <span className="fv">
              <input
                className="inp grow"
                placeholder="e.g. Kissa Master (kissaten)"
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </span>
          </div>
          <div className="field">
            <span className="fl">Kind</span>
            <span className="fv">
              <select className="inp grow" value={kind} onChange={(e) => setKind(e.target.value as PlaceKind)}>
                {(Object.keys(PLACE_KIND_LABEL) as PlaceKind[]).map((k) => (
                  <option key={k} value={k}>
                    {PLACE_KIND_LABEL[k]}
                  </option>
                ))}
              </select>
            </span>
          </div>
          <div className="field">
            <span className="fl">City</span>
            <span className="fv">
              <select className="inp grow" value={city} onChange={(e) => setCity(e.target.value)}>
                {cities.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
            </span>
          </div>
          {coord && (
            <div className="field">
              <span className="fl">Pinned</span>
              <span className="fv">
                <span className="hint">
                  📍 {coord.lat.toFixed(4)}, {coord.lng.toFixed(4)} — from the map
                  {selectedIsTripPlace ? ' · already in the trip, will be reused' : ''}
                </span>
              </span>
            </div>
          )}
          <div className="field">
            <span className="fl">Link</span>
            <span className="fv">
              <input
                className="inp grow"
                placeholder="Google Maps or website (optional)"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
              />
            </span>
          </div>
          <div className="field" style={{ alignItems: 'start' }}>
            <span className="fl">Note</span>
            <span className="fv">
              <textarea
                className="inp grow"
                rows={2}
                placeholder="Anything the group should know (optional)"
                value={note}
                onChange={(e) => setNote(e.target.value)}
              />
            </span>
          </div>
        </>
      )}

      <div className="field">
        <span className="fl">Insert</span>
        <span className="fv">
          <select className="inp grow" value={effectiveSlot} onChange={(e) => setSlot(e.target.value)}>
            {slotChoices.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </span>
      </div>

      <div className="field" style={{ alignItems: 'start' }}>
        <span className="fl">Why</span>
        <span className="fv">
          <textarea
            className="inp grow"
            rows={2}
            placeholder={(mode === 'candidates' ? chosen?.pitch : '') || 'Why this place fits the day…'}
            value={why}
            onChange={(e) => setWhy(e.target.value)}
          />
        </span>
      </div>
    </>
  );

  const embeddedMap = (
    <div className="compose-mappane">
      <MapView
        markers={embedMarkers}
        routes={embedRoutes}
        bounds={embedBounds}
        padding={18}
        onMarkerClick={(id) => {
          if (id.startsWith('cand:')) {
            setMode('candidates');
            setCandidateId(id.slice(5));
          } else if (id.startsWith('sr:')) {
            search.select(id.slice(3));
          }
        }}
      />
    </div>
  );

  return (
    <div className={`compose${docked ? ' compose-docked' : ' compose-hasmap'}`}>
      <div className="compose-head">
        <span className="kd" style={{ background: KIND_COLOR.meal }} />
        <strong>
          Propose a stop · Day {dayIndex + 1} ({activeDay.cityHint})
        </strong>
        <ComposeClose onClose={onClose} />
      </div>

      {docked ? (
        fields
      ) : (
        <div className="compose-split">
          {embeddedMap}
          <div className="compose-form">{fields}</div>
        </div>
      )}

      {ops.length > 0 && (
        <div className="preview">
          <span className="block-h">Preview</span>
          <ChangeList
            ops={ops}
            detail={detail}
            extraPlaces={chosen ? [chosen.place] : selectedResult ? [selectedResult] : []}
          />
          {feasibility &&
            (() => {
              const proj = projectFeasibilityAfterAdd(feasibility.usedMin, feasibility.windowMin);
              if (proj.feasibility === 'ok') return null;
              return (
                <div className="warn">
                  ⚠{' '}
                  <span>
                    Adding it takes Day {dayIndex + 1} to <b>~{Math.round(proj.pct * 100)}%</b> of its window — leaders
                    see this flag before deciding.
                  </span>
                </div>
              );
            })()}
        </div>
      )}

      <RouteSeg value={route} onChange={setRoute} />
      <div className="compose-foot">
        <span className="consequence quiet">Structural — applies on approval.</span>
        <span className="spacer" />
        <button type="button" className="btn" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn solid"
          disabled={!canSubmit || submit.isPending}
          onClick={() => submit.mutate()}
        >
          {route === 'poll' ? 'Open the poll →' : 'Send to leaders →'}
        </button>
      </div>
    </div>
  );
}
