import type { ChangeOp, Day, Feasibility, Place, PlaceKind, PlanDetail, Stop, StopKind } from '../api/types';
import { KIND_COLOR, PLACE_KIND_COLOR } from './planShared';

/**
 * Shared governance rendering: turning a ChangeSet's ops into the plain-English
 * verb-chip change list the mockup approves (ADD/DROP/MOVE/REORDER/SWAP), plus
 * the place-kind → stop-kind mapping the composers need. Used by the proposal
 * cards, the plan-change poll diff, the review queue, and the compose previews.
 */

/** Place kinds become these stop kinds when a candidate is added to the plan. */
export const PLACE_TO_STOP_KIND: Record<PlaceKind, StopKind> = {
  sight: 'visit',
  food: 'meal',
  lodging: 'lodging',
  activity: 'activity',
  transport_hub: 'transit',
};

/** Human labels for the "Somewhere new" place-kind <select>. */
export const PLACE_KIND_LABEL: Record<PlaceKind, string> = {
  sight: 'Sight',
  food: 'Food',
  lodging: 'Lodging',
  activity: 'Activity',
  transport_hub: 'Transport',
};

/**
 * "Where in the day" <select> options — First of the day, then "after <stop>"
 * per existing stop. Value is 'first' or a stopId. Shared by the add-stop
 * Insert picker and the propose-change Move position picker.
 */
export function slotOptions(stops: Stop[], placeName: (placeId: string) => string): { value: string; label: string }[] {
  return [
    { value: 'first', label: 'First of the day' },
    ...stops.map((s) => ({ value: s.id, label: `after ${placeName(s.placeId)}` })),
  ];
}

/** Fractional seq for a chosen slot; MockApiClient.resequence() renumbers to
    integers, so 0.5 lands first and `stop.seq + 0.5` lands right after it. */
export function seqForSlot(value: string, stops: Stop[]): number {
  if (value === 'first') return 0.5;
  const s = stops.find((x) => x.id === value);
  return s ? s.seq + 0.5 : stops.length + 1;
}

/** Visit length a freshly added stop enters at — mirrors MockApiClient.applyOp
    (`durationMin: 60`), reused so the composer's pre-submit projection lines up
    with the feasibility the poll will recompute. */
export const NEW_STOP_VISIT_MIN = 60;

export interface ProjectedFeasibility {
  feasibility: Feasibility;
  pct: number; // projected fraction of the day window used
}

/** A day's feasibility band AFTER one stop is added, from its current load.
    Mirrors MockApiClient.recomputeFeasibility: usedMin = visits + legs banded at
    85%/100%, per-stop visit heuristic NEW_STOP_VISIT_MIN. An add leaves the
    mock's legs untouched, so the projection only adds the new visit minutes. */
export function projectFeasibilityAfterAdd(usedMin: number, windowMin: number): ProjectedFeasibility {
  const pct = (usedMin + NEW_STOP_VISIT_MIN) / windowMin;
  const feasibility: Feasibility = pct > 1 ? 'unreasonable' : pct >= 0.85 ? 'tight' : 'ok';
  return { feasibility, pct };
}

type Verb = 'add' | 'drop' | 'move' | 'reorder' | 'swap';
const VERB_LABEL: Record<Verb, string> = { add: 'Add', drop: 'Drop', move: 'Move', reorder: 'Reorder', swap: 'Swap' };

interface Resolver {
  placeName: (placeId: string) => string;
  placeColor: (placeId: string) => string;
  stopPlaceId: (stopId: string) => string | null;
  stopDayLabel: (stopId: string) => string;
  dayLabel: (dayId: string) => string;
}

function makeResolver(detail: PlanDetail, extraPlaces: Place[]): Resolver {
  const placeById = new Map([...detail.places, ...extraPlaces].map((p) => [p.id, p]));
  const stopById = new Map(detail.stops.map((s) => [s.id, s]));
  const orderedDays = [...detail.days].sort((a, b) => a.date.localeCompare(b.date));
  const dayNumber = new Map(orderedDays.map((d, i) => [d.id, i + 1]));
  return {
    placeName: (placeId) => placeById.get(placeId)?.name ?? placeId,
    placeColor: (placeId) => {
      const kind = placeById.get(placeId)?.kind;
      return kind ? PLACE_KIND_COLOR[kind] : KIND_COLOR.visit;
    },
    stopPlaceId: (stopId) => stopById.get(stopId)?.placeId ?? null,
    stopDayLabel: (stopId) => {
      const dayId = stopById.get(stopId)?.dayId;
      return dayId ? `Day ${dayNumber.get(dayId) ?? '?'}` : 'the plan';
    },
    dayLabel: (dayId) => `Day ${dayNumber.get(dayId) ?? '?'}`,
  };
}

function Dot({ color }: { color: string }) {
  return <span className="dot" style={{ background: color }} />;
}

function opRow(op: ChangeOp, i: number, r: Resolver) {
  switch (op.op) {
    case 'add_stop':
      return (
        <Chg key={i} verb="add">
          <Dot color={r.placeColor(op.placeId)} />
          <span className="place">{r.placeName(op.placeId)}</span> <span className="arrow">→</span>{' '}
          {r.dayLabel(op.dayId)}, slot {Math.ceil(op.seq)}
        </Chg>
      );
    case 'add_place_stop':
      // A place that doesn't exist yet — resolve from the draft, not the catalog.
      return (
        <Chg key={i} verb="add">
          <Dot color={PLACE_KIND_COLOR[op.draft.kind]} />
          <span className="place">{op.draft.name}</span>{' '}
          <span className="from">
            (new · {op.draft.city}
            {op.draft.lat != null && op.draft.lng != null ? ' · 📍 pinned' : ''})
          </span>{' '}
          <span className="arrow">→</span> {r.dayLabel(op.dayId)}, slot {Math.ceil(op.seq)}
        </Chg>
      );
    case 'remove_stop': {
      const placeId = r.stopPlaceId(op.stopId);
      return (
        <Chg key={i} verb="drop">
          {placeId && <Dot color={r.placeColor(placeId)} />}
          <span className="place">{placeId ? r.placeName(placeId) : op.stopId}</span>{' '}
          <span className="from">— from {r.stopDayLabel(op.stopId)}</span>
        </Chg>
      );
    }
    case 'move_stop': {
      const placeId = r.stopPlaceId(op.stopId);
      return (
        <Chg key={i} verb="move">
          {placeId && <Dot color={r.placeColor(placeId)} />}
          <span className="place">{placeId ? r.placeName(placeId) : op.stopId}</span>{' '}
          <span className="from">{r.stopDayLabel(op.stopId)}</span> <span className="arrow">→</span>{' '}
          {r.dayLabel(op.toDayId)}, slot {Math.ceil(op.seq)}
        </Chg>
      );
    }
    case 'reorder': {
      const order = op.stopIdsInOrder
        .map((sid) => {
          const pid = r.stopPlaceId(sid);
          return pid ? r.placeName(pid) : sid;
        })
        .join(' → ');
      return (
        <Chg key={i} verb="reorder">
          <span className="from">{r.dayLabel(op.dayId)}:</span> {order}
        </Chg>
      );
    }
    case 'swap_place': {
      const placeId = r.stopPlaceId(op.stopId);
      return (
        <Chg key={i} verb="swap">
          {placeId && <Dot color={r.placeColor(placeId)} />}
          <span className="from">{placeId ? r.placeName(placeId) : op.stopId}</span> <span className="arrow">→</span>{' '}
          <span className="place">{r.placeName(op.newPlaceId)}</span>
        </Chg>
      );
    }
    case 'add_day':
      return (
        <Chg key={i} verb="add">
          <span className="place">New day</span> — {op.date} · {op.cityHint}
        </Chg>
      );
    case 'remove_day':
      return (
        <Chg key={i} verb="drop">
          <span className="place">{r.dayLabel(op.dayId)}</span> removed
        </Chg>
      );
  }
}

function Chg({ verb, children }: { verb: Verb; children: React.ReactNode }) {
  return (
    <div className="chg">
      <span className={`verb ${verb}`}>{VERB_LABEL[verb]}</span>
      <span className="txt">{children}</span>
    </div>
  );
}

/** Render a ChangeSet's ops as the approved human-readable diff. `detail` is the
    live plan, used to resolve stop ids to place names and day numbers. */
export function ChangeList({
  ops,
  detail,
  extraPlaces,
  className,
}: {
  ops: ChangeOp[];
  detail: PlanDetail;
  extraPlaces?: Place[];
  className?: string;
}) {
  const r = makeResolver(detail, extraPlaces ?? []);
  return <div className={`changes${className ? ` ${className}` : ''}`}>{ops.map((op, i) => opRow(op, i, r))}</div>;
}

/** Day dropdown options ("Day 5 · Wed 18") for the propose-change composer. */
export function dayOptionLabel(day: Day, index: number): string {
  const d = new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' });
  return `Day ${index + 1} · ${d} · ${day.cityHint}`;
}
