import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useSearchParams } from 'react-router';
import { useApi } from '../api/useApi';
import { invalidateTripPlanning } from '../api/queryInvalidation';
import { useIsDesktop } from '../components/hooks';
import { useModalChrome } from '../components/useModalChrome';
import { useI18n } from '../i18n';
import { fillStyle } from '../lib/oklch';
import { MapView } from '../map/MapView';
import type { LngLat } from '../map/MapRenderer';
import { KIND_COLOR, PLACE_KIND_COLOR, PLACE_KIND_STOP_KIND } from './planShared';
import { ChangeList } from './governanceShared';
import {
  PLACE_KINDS,
  dayOptionLabel,
  localizedPlaceKind,
  projectFeasibilityAfterAdd,
  seqForSlot,
  slotOptions,
} from './governanceDomain';
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
import { readAddStopDeepLink, stripAddStopDeepLink } from './planDeepLinks';
import type { StopMode } from './planDeepLinks';
import type { GovAction } from './planActions';
import { useStopSearch } from './useStopSearch';
import type { StopSearchController } from './useStopSearch';
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

export interface GovData {
  tripId: string;
  detail: PlanDetail;
  days: Day[];
  candidates: CandidateWithPlace[];
  membersById: Map<string, User>;
  threads: Thread[];
  isLeader: boolean;
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
              isLeader={data.isLeader}
              onClose={requestClose}
            />
          )}
          {action.kind === 'addStop' && (
            <ProposeStopComposer
              day={action.day}
              initialSlot={action.initialSlot}
              initialCandidateId={action.initialCandidateId}
              allowDaySelection={action.allowDaySelection}
              detail={data.detail}
              days={data.days}
              candidates={data.candidates}
              tripId={data.tripId}
              isLeader={data.isLeader}
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
  const chrome = useModalChrome<HTMLDivElement>();
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
        ref={chrome}
        className={`gov-modal${isDesktop ? '' : ' sheet'}${wide ? ' wide' : ''}`}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        /* Every compose surface renders its title into `.compose-head h3`, so
           one id here names all of them. Without it the dialog announced as
           just "dialog". */
        aria-labelledby="gov-modal-title"
        tabIndex={-1}
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
  const { t, formatDate } = useI18n();
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
      queryClient.invalidateQueries({ queryKey: ['threads', tripId] });
    },
  });

  const comments = useQuery({
    queryKey: ['comments', tripId, thread?.id],
    queryFn: () => api.getComments(tripId, thread!.id),
    enabled: !!thread,
  });
  const post = useMutation({
    mutationFn: (body: string) => api.addComment(tripId, thread!.id, body),
    onSuccess: () => {
      setDraft('');
      queryClient.invalidateQueries({ queryKey: ['comments', tripId, thread?.id] });
      queryClient.invalidateQueries({ queryKey: ['threads', tripId] });
    },
  });
  const react = useMutation({
    mutationFn: ({ commentId, emoji, active }: { commentId: string; emoji: string; active: boolean }) =>
      api.setReaction(tripId, thread!.id, commentId, emoji, active),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['comments', tripId, thread?.id] }),
  });

  return (
    <div className="panel-card">
      <div className="panel-top">
        <span className="anchor" id="gov-modal-title">
          <span className="kd" style={{ background: KIND_COLOR[stop.stopKind] }} />
          {t('plan.gov.dayAnchor', { place: place?.name ?? stop.placeId, day: dayIndex + 1 })}
        </span>
        <button type="button" className="close" onClick={onClose} aria-label={t('plan.gov.close')}>
          ✕
        </button>
      </div>
      {thread ? (
        <>
          <div className="thread-title">{thread.title}</div>
          <div className="thread-body">
            {comments.isLoading && <p className="muted">{t('plan.gov.loading')}</p>}
            {(comments.data ?? []).map((c) => {
              const author = membersById.get(c.author);
              const mine = c.author === me.data?.id;
              return (
                <div key={c.id} className={`cmt${mine ? ' me' : ''}`}>
                  <span className="avatar sm" style={fillStyle(author?.avatarColor ?? '#888')}>
                    {author?.displayName[0] ?? '?'}
                  </span>
                  <div>
                    <div className="bubble">
                      <div className="ch">
                        <span className="nm">{author?.displayName ?? '—'}</span>
                        <span className="tm">{formatDate(c.createdAt, { day: 'numeric', month: 'short' })}</span>
                      </div>
                      <div className="bd">{renderEmphasis(c.body)}</div>
                    </div>
                    <div className="rxn">
                      {c.reactions.map((r) => {
                        const onIt = r.userIds.includes(me.data?.id ?? '');
                        return (
                          <button
                            key={r.emoji}
                            type="button"
                            className={`r${onIt ? ' on' : ''}`}
                            /* Toggle, not a command — without this the accent
                               ring is the only thing saying you already
                               reacted, and a screen reader sees none of it. */
                            aria-pressed={onIt}
                            aria-label={t(
                              r.userIds.length === 1 ? 'plan.gov.reactionPerson' : 'plan.gov.reactionPeople',
                              { emoji: r.emoji, count: r.userIds.length },
                            )}
                            onClick={() => react.mutate({ commentId: c.id, emoji: r.emoji, active: !onIt })}
                          >
                            {r.emoji} {r.userIds.length}
                          </button>
                        );
                      })}
                      <button
                        type="button"
                        className="r add"
                        aria-label={t('plan.gov.reactThumbsUp')}
                        onClick={() => react.mutate({ commentId: c.id, emoji: '👍', active: true })}
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
            <span className="avatar sm" style={fillStyle(me.data?.avatarColor ?? '#6b5bd2')}>
              {me.data?.displayName[0] ?? 'K'}
            </span>
            <input
              className="in"
              placeholder={t('plan.gov.addToThread')}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
            <button className="btn solid sm" type="submit" disabled={!draft.trim() || post.isPending}>
              {t('plan.gov.send')}
            </button>
          </form>
        </>
      ) : (
        <>
          <div className="thread-title">{t('plan.gov.discussion')}</div>
          <div className="thread-body">
            {/* The old copy — "kick one off. It threads under X" — described the
                data model and left the reader with nothing to type. An empty
                state's job is to hand you the first sentence. */}
            <div className="thread-empty">
              <p>{t('plan.gov.noDiscussion', { place: place?.name ?? stop.placeId })}</p>
              <ul>
                <li>{t('plan.gov.discussionPromptWants')}</li>
                <li>{t('plan.gov.discussionPromptClash')}</li>
                <li>{t('plan.gov.discussionPromptTime')}</li>
              </ul>
            </div>
          </div>
          <form
            className="composer start"
            onSubmit={(e) => {
              e.preventDefault();
              if (startDraft.trim()) start.mutate(startDraft.trim());
            }}
          >
            <span className="avatar sm" style={fillStyle(me.data?.avatarColor ?? '#6b5bd2')}>
              {me.data?.displayName[0] ?? 'K'}
            </span>
            <textarea
              className="in"
              rows={2}
              placeholder={t('plan.gov.startDiscussion')}
              value={startDraft}
              onChange={(e) => setStartDraft(e.target.value)}
            />
            <button className="btn solid sm" type="submit" disabled={!startDraft.trim() || start.isPending}>
              {t('plan.gov.start')}
            </button>
          </form>
        </>
      )}
    </div>
  );
}

/* ═══════════════ shared composer bits ═══════════════ */

/* Route is one required choice, so use native radios instead of styling two
   buttons as a segmented control. This gives keyboard users arrow-key movement
   for free and makes the unselected option read as available, not disabled. */
function RouteSeg({
  value,
  onChange,
  isLeader,
}: {
  value: ProposalRoute;
  onChange: (r: ProposalRoute) => void;
  isLeader: boolean;
}) {
  const { t } = useI18n();
  const id = useId();
  const labelId = `${id}-label`;
  const name = `${id}-route`;
  const directLabel = t(isLeader ? 'plan.gov.applyNow' : 'plan.gov.leaderApproval');
  const directDescription = t(isLeader ? 'plan.gov.applyNowDescription' : 'plan.gov.leaderApprovalDescription');
  return (
    <fieldset className="compose-route">
      <legend className="fl" id={labelId}>
        {t('plan.gov.route')}
      </legend>
      <div className="route-seg" role="radiogroup" aria-labelledby={labelId}>
        <label className={`route-option${value === 'leader_approval' ? ' active' : ''}`}>
          <input
            type="radio"
            name={name}
            value="leader_approval"
            checked={value === 'leader_approval'}
            onChange={() => onChange('leader_approval')}
            aria-label={directLabel}
          />
          <span className="route-option-mark" aria-hidden />
          <span className="route-option-copy">
            <strong>{directLabel}</strong>
            <small>{directDescription}</small>
          </span>
        </label>
        <label className={`route-option${value === 'poll' ? ' active' : ''}`}>
          <input
            type="radio"
            name={name}
            value="poll"
            checked={value === 'poll'}
            onChange={() => onChange('poll')}
            aria-label={t('plan.gov.openPoll')}
          />
          <span className="route-option-mark" aria-hidden />
          <span className="route-option-copy">
            <strong>{t('plan.gov.openPoll')}</strong>
            <small>{t('plan.gov.openPollDescription')}</small>
          </span>
        </label>
      </div>
    </fieldset>
  );
}

/** Header ✕ shared by every composer — closes the surface. */
function ComposeClose({ onClose }: { onClose: () => void }) {
  const { t } = useI18n();
  return (
    <button type="button" className="compose-x" onClick={onClose} aria-label={t('plan.gov.close')}>
      ✕
    </button>
  );
}

function Sent({ route, isLeader, onClose }: { route: ProposalRoute; isLeader: boolean; onClose: () => void }) {
  const { t } = useI18n();
  const titleKey =
    route === 'poll' ? 'plan.gov.pollOpened' : isLeader ? 'plan.gov.planPublished' : 'plan.gov.sentLeaders';
  const bodyKey =
    route === 'poll' ? 'plan.gov.pollOpenedBody' : isLeader ? 'plan.gov.planPublishedBody' : 'plan.gov.sentLeadersBody';
  const trackingKey =
    route === 'poll' ? 'plan.gov.trackPoll' : isLeader ? 'plan.gov.trackPublished' : 'plan.gov.trackApproval';
  return (
    <div className="compose sent">
      <strong id="gov-modal-title">{t(titleKey)}</strong>
      <p className="muted">
        {t(bodyKey)} {t(trackingKey)}
      </p>
      <div className="compose-foot">
        <span className="spacer" />
        <button type="button" className="btn solid" onClick={onClose}>
          {t('plan.gov.done')}
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
  isLeader,
  onClose,
}: {
  stop: Stop;
  detail: PlanDetail;
  days: Day[];
  tripId: string;
  isLeader: boolean;
  onClose: () => void;
}) {
  const { t, formatDate } = useI18n();
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
  const [sentRoute, setSentRoute] = useState<ProposalRoute | null>(null);

  const toIndex = ordered.findIndex((d) => d.id === toDayId);
  // Target-day slot options exclude the stop itself so you can't drop it after
  // where it already sits. Default: end of the target day.
  const targetStops = detail.stops.filter((s) => s.dayId === toDayId && s.id !== stop.id).sort((a, b) => a.seq - b.seq);
  const slotChoices = slotOptions(targetStops, placeName, t);
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
    mutationFn: (submittedRoute: ProposalRoute) =>
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
        route: submittedRoute,
      }),
    onSuccess: (_proposal, submittedRoute) => {
      setSentRoute(submittedRoute);
      return invalidateTripPlanning(queryClient, tripId);
    },
  });

  if (sentRoute) return <Sent route={sentRoute} isLeader={isLeader} onClose={onClose} />;

  return (
    <div className="compose">
      <div className="compose-head">
        <span className="kd" style={{ background: KIND_COLOR[stop.stopKind] }} />
        <strong id="gov-modal-title">{t('plan.gov.proposeChange', { place: place?.name ?? stop.placeId })}</strong>
        <span className="badge">{t('plan.dayNumber', { day: currentIndex + 1 })}</span>
        <ComposeClose onClose={onClose} />
      </div>

      {/* Only this band scrolls — see the `.gov-modal` comment in index.css. */}
      <div className="compose-body">
        <div className="field">
          <span className="fl">{t('plan.gov.action')}</span>
          <span className="fv">
            <span className="route-seg">
              <button
                type="button"
                className={mode === 'move' ? 'active' : ''}
                aria-pressed={mode === 'move'}
                onClick={() => setMode('move')}
              >
                {t('plan.gov.move')}
              </button>
              <button
                type="button"
                className={mode === 'remove' ? 'active' : ''}
                aria-pressed={mode === 'remove'}
                onClick={() => setMode('remove')}
              >
                {t('plan.gov.remove')}
              </button>
            </span>
          </span>
        </div>

        {mode === 'move' ? (
          <>
            <div className="field">
              <span className="fl">{t('plan.gov.moveToDay')}</span>
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
                      {dayOptionLabel(d, i, formatDate, t)}
                    </option>
                  ))}
                </select>
              </span>
            </div>
            <div className="field">
              <span className="fl">{t('plan.gov.position')}</span>
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
              <span className="fl">{t('plan.gov.plannedArrival')}</span>
              <span className="fv">
                <span className="inp was">{stop.plannedArrival}</span>
                <span className="hint">{t('plan.gov.timeAfterMove')}</span>
              </span>
            </div>
          </>
        ) : (
          <div className="field">
            <span className="fl">{t('plan.gov.dropping')}</span>
            <span className="fv">
              <span className="inp was">{place?.name}</span>
              <span className="hint">{t('plan.gov.removesFromDay', { day: currentIndex + 1 })}</span>
            </span>
          </div>
        )}

        <div className="field" style={{ alignItems: 'start' }}>
          <span className="fl">{t(mode === 'remove' ? 'plan.gov.whyRequired' : 'plan.gov.why')}</span>
          <span className="fv">
            <textarea
              className="inp grow"
              rows={2}
              placeholder={mode === 'remove' ? t('plan.gov.removeWhyPlaceholder') : t('plan.gov.moveWhyPlaceholder')}
              value={why}
              onChange={(e) => setWhy(e.target.value)}
            />
          </span>
        </div>

        {ops.length > 0 ? (
          <div className="preview">
            <span className="block-h">
              {t(
                route === 'poll'
                  ? 'plan.gov.previewPoll'
                  : isLeader
                    ? 'plan.gov.previewPublish'
                    : 'plan.gov.previewLeaders',
              )}
            </span>
            <ChangeList ops={ops} detail={detail} />
          </div>
        ) : mode === 'move' ? (
          <div className="warn">
            ⚠ <span>{t('plan.gov.samePositionWarning')}</span>
          </div>
        ) : (
          <div className="warn">
            ⚠ <span>{t('plan.gov.removeReasonWarning')}</span>
          </div>
        )}
      </div>

      <div className="compose-dock">
        <RouteSeg value={route} onChange={setRoute} isLeader={isLeader} />
        <div className="compose-foot">
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>
            {t('plan.gov.cancel')}
          </button>
          <button
            type="button"
            className="btn solid"
            disabled={!canSubmit || submit.isPending}
            onClick={() => submit.mutate(route)}
          >
            {t(
              route === 'poll'
                ? 'plan.gov.openPollAction'
                : isLeader
                  ? 'plan.gov.applyNowAction'
                  : 'plan.gov.sendLeadersAction',
            )}
          </button>
        </div>
      </div>
    </div>
  );
}

/* ═══════════════ Propose a stop (candidates | somewhere new) ═══════════════ */

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
 * Selecting a search hit prefills name/kind/city/coordinates. Catalog hits stay
 * editable before they become a new trip place; a hit that's already a trip
 * place is clearly reused as-is via `add_stop` instead of pretending edits to
 * its saved details will be applied. Manual entry works when nothing is found.
 */
export function ProposeStopComposer({
  day,
  initialSlot,
  initialCandidateId,
  allowDaySelection,
  detail,
  days,
  candidates,
  tripId,
  isLeader,
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
  /** Initial insertion point supplied by a contextual free-time region. */
  initialSlot?: string;
  /** Candidate-card entry points can preselect their own idea without a URL hop. */
  initialCandidateId?: string;
  /** Candidate-card entry points are not tied to a day, so the user chooses one. */
  allowDaySelection?: boolean;
  detail: PlanDetail;
  days: Day[];
  candidates: CandidateWithPlace[];
  tripId: string;
  isLeader: boolean;
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
  const { locale, t, formatDate } = useI18n();
  const api = useApi();
  const queryClient = useQueryClient();
  const [urlParams, setUrlParams] = useSearchParams();
  const cityFieldId = useId();
  const cityListId = `${cityFieldId}-suggestions`;
  const cityHintId = `${cityFieldId}-hint`;
  const reuseNoteId = `${cityFieldId}-reuse`;
  const placeName = (id: string) => detail.places.find((p) => p.id === id)?.name ?? id;
  const orderedDays = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const shortlisted = candidates.filter((c) => c.status === 'shortlisted');

  // Opened without a fixed day (candidate → plan deep link): let the composer
  // pick the day itself. From a day's "＋ Propose a stop" (or a `day=` link) the
  // day is fixed and this select never appears.
  const [pickDay] = useState(() => {
    if (allowDaySelection) return true;
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
  const [candidateIdI, setCandidateIdI] = useState(initialCandidateId ?? shortlisted[0]?.id ?? '');
  const mode = modeProp ?? modeI;
  const setMode = onModeChange ?? setModeI;
  const candidateId = candidateIdProp ?? candidateIdI;
  const setCandidateId = onCandidateChange ?? setCandidateIdI;

  // Search: the shell's controller when docked, otherwise our own. (The hook
  // still runs when a prop is supplied; with an empty query it does nothing.)
  const ownSearch = useStopSearch(tripId);
  const search = searchProp ?? ownSearch;

  // New-place draft + insert slot are always local to the composer.
  const [slot, setSlot] = useState<string>(initialSlot ?? '');
  const [why, setWhy] = useState('');
  const [name, setName] = useState('');
  const [kind, setKind] = useState<PlaceKind>('sight');
  const [city, setCity] = useState(day.cityHint);
  const [note, setNote] = useState('');
  const [url, setUrl] = useState('');
  const [coord, setCoord] = useState<LngLat | null>(null); // set when a search hit is picked
  const [route, setRoute] = useState<ProposalRoute>('poll');
  const [sentRoute, setSentRoute] = useState<ProposalRoute | null>(null);

  // Picking a catalog hit prefills a draft that remains editable. Existing trip
  // places are rendered as a locked reuse summary below, but keeping their
  // canonical values in state makes "clear selection · enter by hand" useful.
  const lastPrefilled = useRef<string | null>(null);
  useEffect(() => {
    const sel = search.selected;
    if (sel && sel.id !== lastPrefilled.current) {
      lastPrefilled.current = sel.id;
      setName(sel.name);
      setKind(sel.kind);
      setCity(sel.city);
      setUrl(sel.website ?? '');
      setNote('');
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

  const slotChoices = slotOptions(dayStops, placeName, t);
  const effectiveSlot = slot || slotChoices[slotChoices.length - 1].value;
  const seq = seqForSlot(effectiveSlot, dayStops);

  const chosen = shortlisted.find((c) => c.id === candidateId);
  const trimmedName = name.trim();
  const trimmedCity = city.trim();
  const selectedResult = search.selected;
  // A hit that's already in the plan is re-added by reference, not re-minted.
  const selectedIsTripPlace = !!selectedResult && detail.places.some((p) => p.id === selectedResult.id);
  const reusedPlace = selectedIsTripPlace ? selectedResult : null;
  const citySuggestions = [
    ...new Set(
      [...orderedDays.map((d) => d.cityHint), ...detail.places.map((p) => p.city), ...search.results.map((p) => p.city)]
        .map((value) => value.trim())
        .filter(Boolean),
    ),
  ];
  const newDraft: NewPlaceDraft = {
    name: trimmedName,
    kind,
    city: trimmedCity,
    note: note.trim(),
    url: url.trim() || null,
    lat: coord?.lat ?? null,
    lng: coord?.lng ?? null,
  };

  const ops: ChangeOp[] =
    mode === 'new'
      ? reusedPlace
        ? [
            {
              op: 'add_stop',
              dayId: activeDay.id,
              placeId: reusedPlace.id,
              seq,
              stopKind: PLACE_KIND_STOP_KIND[reusedPlace.kind],
            },
          ]
        : trimmedName && trimmedCity
          ? [{ op: 'add_place_stop', dayId: activeDay.id, seq, stopKind: PLACE_KIND_STOP_KIND[kind], draft: newDraft }]
          : []
      : chosen
        ? [
            {
              op: 'add_stop',
              dayId: activeDay.id,
              placeId: chosen.placeId,
              seq,
              stopKind: PLACE_KIND_STOP_KIND[chosen.place.kind],
            },
          ]
        : [];

  const canSubmit = mode === 'new' ? !!reusedPlace || (trimmedName.length > 0 && trimmedCity.length > 0) : !!chosen;
  const addedName = mode === 'new' ? reusedPlace?.name || trimmedName || 'a place' : (chosen?.place.name ?? 'a stop');

  // Insert-outcome preview: where the picked place lands (a candidate's coords,
  // or a search-hit / map-pinned coord in "new" mode) and its resulting 1-based
  // stop number. `seq` is fractional (0.5 lands first); the integer index is how
  // many stops sit before it. A hand-entered place with no coordinates has
  // nothing to place, so there is no outcome to draw.
  const insertAt = useMemo<LngLat | null>(
    () => (mode === 'candidates' ? (chosen ? { lng: chosen.place.lng, lat: chosen.place.lat } : null) : coord),
    [chosen, coord, mode],
  );
  const previewSeq = dayStops.filter((s) => s.seq < seq).length + 1;
  const previewAt = ops.length > 0 ? insertAt : null;

  const submit = useMutation({
    mutationFn: (submittedRoute: ProposalRoute) =>
      api.createProposal(tripId, {
        title: `Add ${addedName} to Day ${dayIndex + 1}`,
        rationale:
          why.trim() || (mode === 'candidates' ? chosen?.pitch : '') || `Add ${addedName} to Day ${dayIndex + 1}.`,
        changeSet: { basePlanVersion: detail.plan.version, ops },
        route: submittedRoute,
      }),
    onSuccess: (_proposal, submittedRoute) => {
      setSentRoute(submittedRoute);
      return invalidateTripPlanning(queryClient, tripId);
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
    const base = dayMarkers(dayGeo, null, mode === 'candidates', locale, pick);
    const withHits =
      mode === 'new' ? [...base, ...searchResultMarkers(search.results, search.selectedId, locale)] : base;
    return previewAt ? [...withHits, proposedStopMarker(previewAt, previewSeq, locale)] : withHits;
  }, [dayGeo, mode, candidateId, search.results, search.selectedId, previewAt, previewSeq, locale]);
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
  }, [dayGeo, mode, search.results, previewAt]);
  const embedRoutes = useMemo(
    () => (previewAt ? proposedDayRoutes(dayGeo, previewAt, previewSeq) : dayRoutes(dayGeo)),
    [dayGeo, previewAt, previewSeq],
  );

  // Docked: hand the insert-outcome preview to the shell so it lands on the live
  // map. Fire on change, and clear on unmount so a closed composer leaves no pin.
  useEffect(() => {
    onPreviewChange?.(previewAt ? { insertAt: previewAt, seq: previewSeq } : null);
  }, [onPreviewChange, previewAt, previewSeq]);
  useEffect(() => () => onPreviewChange?.(null), [onPreviewChange]);

  if (sentRoute) return <Sent route={sentRoute} isLeader={isLeader} onClose={onClose} />;

  const searchBox = (
    <div className="field" style={{ alignItems: 'start' }}>
      <span className="fl">{t('plan.gov.search')}</span>
      <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
        <input
          className="inp grow"
          placeholder={t('plan.gov.searchPlaceholder')}
          value={search.query}
          onChange={(e) => search.setQuery(e.target.value)}
        />
        {search.query.trim() && (
          <div className="place-results">
            {search.loading && <span className="muted pr-status">{t('plan.gov.searching')}</span>}
            {!search.loading && search.results.length === 0 && (
              <span className="muted pr-status">{t('plan.gov.noMatches')}</span>
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
                    {localizedPlaceKind(r.kind, t)} · {r.city}
                  </span>
                </span>
                {detail.places.some((p) => p.id === r.id) && <span className="badge">{t('plan.gov.inTrip')}</span>}
              </button>
            ))}
          </div>
        )}
        {selectedResult && (
          <button type="button" className="clear-sel" onClick={clearSelection}>
            {t('plan.gov.clearSelection')}
          </button>
        )}
        {docked && <span className="hint">{t('plan.gov.searchPinsHint')}</span>}
      </span>
    </div>
  );

  const fields = (
    <>
      {pickDay && (
        <div className="field">
          <span className="fl">{t('plan.day')}</span>
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
                  {dayOptionLabel(d, i, formatDate, t)}
                </option>
              ))}
            </select>
          </span>
        </div>
      )}
      <div className="field">
        <span className="fl">{t('plan.gov.add')}</span>
        <span className="fv">
          <span className="route-seg">
            <button
              type="button"
              className={mode === 'candidates' ? 'active' : ''}
              aria-pressed={mode === 'candidates'}
              onClick={() => setMode('candidates')}
            >
              {t('plan.gov.tripIdeas')}
            </button>
            <button
              type="button"
              className={mode === 'new' ? 'active' : ''}
              aria-pressed={mode === 'new'}
              onClick={() => setMode('new')}
            >
              {t('plan.gov.searchPlaces')}
            </button>
          </span>
        </span>
      </div>

      {mode === 'candidates' ? (
        <div className="field" style={{ alignItems: 'start' }}>
          <span className="fl">{t('plan.gov.chooseIdea')}</span>
          <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
            <div className="cand-pick">
              {shortlisted.length === 0 && <span className="muted">{t('plan.gov.noIdeas')}</span>}
              {shortlisted.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  className={`cand-opt${c.id === candidateId ? ' sel' : ''}`}
                  aria-pressed={c.id === candidateId}
                  style={{ '--kc': PLACE_KIND_COLOR[c.place.kind] } as CSSProperties}
                  onClick={() => setCandidateId(c.id)}
                >
                  <span className="rg" />
                  {c.place.name}
                </button>
              ))}
            </div>
            <span className="hint">{t('plan.gov.selectIdeaHint')}</span>
          </span>
        </div>
      ) : (
        <>
          {searchBox}
          {reusedPlace ? (
            <div className="reuse-place-note" id={reuseNoteId} role="note">
              <span className="reuse-place-mark" aria-hidden>
                ↻
              </span>
              <span className="reuse-place-copy">
                <strong>{t('plan.gov.reusePlaceTitle', { place: reusedPlace.name })}</strong>
                <span>{t('plan.gov.reusePlaceHelp')}</span>
                <span className="reuse-place-facts">
                  {localizedPlaceKind(reusedPlace.kind, t)} · {reusedPlace.city}
                </span>
              </span>
            </div>
          ) : (
            <>
              <div className="field">
                <span className="fl">{t('plan.gov.nameRequired')}</span>
                <span className="fv">
                  <input
                    className="inp grow"
                    placeholder={t('plan.gov.namePlaceholder')}
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                  />
                </span>
              </div>
              <div className="field">
                <span className="fl">{t('plan.gov.kind')}</span>
                <span className="fv">
                  <select className="inp grow" value={kind} onChange={(e) => setKind(e.target.value as PlaceKind)}>
                    {PLACE_KINDS.map((k) => (
                      <option key={k} value={k}>
                        {localizedPlaceKind(k, t)}
                      </option>
                    ))}
                  </select>
                </span>
              </div>
              <div className="field">
                <label className="fl" htmlFor={cityFieldId}>
                  {t('plan.gov.cityRequired')}
                </label>
                <span className="fv" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
                  <input
                    id={cityFieldId}
                    className="inp grow"
                    type="text"
                    list={cityListId}
                    autoComplete="address-level2"
                    placeholder={t('plan.gov.cityPlaceholder')}
                    aria-describedby={cityHintId}
                    value={city}
                    onChange={(e) => setCity(e.target.value)}
                    required
                  />
                  <datalist id={cityListId}>
                    {citySuggestions.map((suggestion) => (
                      <option key={suggestion} value={suggestion} />
                    ))}
                  </datalist>
                  <span className="hint" id={cityHintId}>
                    {t('plan.gov.cityHint')}
                  </span>
                </span>
              </div>
            </>
          )}
          {coord && (
            <div className="field">
              <span className="fl">{t('plan.gov.pinned')}</span>
              <span className="fv">
                <span className="hint">
                  📍 {coord.lat.toFixed(4)}, {coord.lng.toFixed(4)} — {t('plan.gov.fromMap')}
                  {selectedIsTripPlace ? ` · ${t('plan.gov.reusePlace')}` : ''}
                </span>
              </span>
            </div>
          )}
          {!reusedPlace && (
            <>
              <div className="field">
                <span className="fl">{t('plan.gov.link')}</span>
                <span className="fv">
                  <input
                    className="inp grow"
                    placeholder={t('plan.gov.linkPlaceholder')}
                    value={url}
                    onChange={(e) => setUrl(e.target.value)}
                  />
                </span>
              </div>
              <div className="field" style={{ alignItems: 'start' }}>
                <span className="fl">{t('plan.gov.note')}</span>
                <span className="fv">
                  <textarea
                    className="inp grow"
                    rows={2}
                    placeholder={t('plan.gov.notePlaceholder')}
                    value={note}
                    onChange={(e) => setNote(e.target.value)}
                  />
                </span>
              </div>
            </>
          )}
        </>
      )}

      <div className="field">
        <span className="fl">{t('plan.gov.insert')}</span>
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
        <span className="fl">{t('plan.gov.why')}</span>
        <span className="fv">
          <textarea
            className="inp grow"
            rows={2}
            placeholder={(mode === 'candidates' ? chosen?.pitch : '') || t('plan.gov.whyFitsPlaceholder')}
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
        <strong id="gov-modal-title">
          {t('plan.gov.proposeStopTitle', { day: dayIndex + 1, city: activeDay.cityHint })}
        </strong>
        <ComposeClose onClose={onClose} />
      </div>

      {/* Only this band scrolls — see the `.gov-modal` comment in index.css.
          Docked (map side panel) it is an ordinary block: the panel is the
          scroller there, so `.compose-body` never gets a height to shrink into
          and simply lays the fields out. */}
      <div className="compose-body">
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
            <span className="block-h">{t('plan.gov.preview')}</span>
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
                      {t('plan.gov.feasibilityWarning', {
                        day: dayIndex + 1,
                        percent: Math.round(proj.pct * 100),
                      })}
                    </span>
                  </div>
                );
              })()}
          </div>
        )}
      </div>

      <div className="compose-dock">
        <RouteSeg value={route} onChange={setRoute} isLeader={isLeader} />
        <div className="compose-foot">
          <span className="consequence quiet">
            {t(
              route === 'poll'
                ? 'plan.gov.structuralPoll'
                : isLeader
                  ? 'plan.gov.structuralPublish'
                  : 'plan.gov.structuralApproval',
            )}
          </span>
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>
            {t('plan.gov.cancel')}
          </button>
          <button
            type="button"
            className="btn solid"
            disabled={!canSubmit || submit.isPending}
            onClick={() => submit.mutate(route)}
          >
            {t(
              route === 'poll'
                ? 'plan.gov.openPollAction'
                : isLeader
                  ? 'plan.gov.applyNowAction'
                  : 'plan.gov.sendLeadersAction',
            )}
          </button>
        </div>
      </div>
    </div>
  );
}
