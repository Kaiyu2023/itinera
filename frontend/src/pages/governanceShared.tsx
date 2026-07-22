import type { ChangeOp, Day, Place, PlaceKind, PlanDetail, StopKind } from '../api/types';
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
          <span className="place">{r.placeName(op.placeId)}</span> <span className="arrow">→</span> {r.dayLabel(op.dayId)}, slot {op.seq}
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
          <span className="from">{r.stopDayLabel(op.stopId)}</span> <span className="arrow">→</span> {r.dayLabel(op.toDayId)}, slot {op.seq}
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
export function ChangeList({ ops, detail, extraPlaces, className }: { ops: ChangeOp[]; detail: PlanDetail; extraPlaces?: Place[]; className?: string }) {
  const r = makeResolver(detail, extraPlaces ?? []);
  return <div className={`changes${className ? ` ${className}` : ''}`}>{ops.map((op, i) => opRow(op, i, r))}</div>;
}

/** Day dropdown options ("Day 5 · Wed 18") for the propose-change composer. */
export function dayOptionLabel(day: Day, index: number): string {
  const d = new Date(day.date + 'T00:00:00').toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' });
  return `Day ${index + 1} · ${d} · ${day.cityHint}`;
}
