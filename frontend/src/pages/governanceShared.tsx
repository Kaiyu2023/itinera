import type { ChangeOp, Place, PlanDetail } from '../api/types';
import { useI18n } from '../i18n';
import { KIND_COLOR, PLACE_KIND_COLOR } from './planShared';
import type { PlanTranslate } from './governanceDomain';

/**
 * Shared governance rendering: turning a ChangeSet's ops into the plain-English
 * verb-chip change list the mockup approves (ADD/DROP/MOVE/REORDER/SWAP), plus
 * the place-kind → stop-kind mapping the composers need. Used by the proposal
 * cards, the plan-change poll diff, the review queue, and the compose previews.
 */

type Verb = 'add' | 'drop' | 'move' | 'reorder' | 'swap';
const VERB_KEY = {
  add: 'plan.change.add',
  drop: 'plan.change.drop',
  move: 'plan.change.move',
  reorder: 'plan.change.reorder',
  swap: 'plan.change.swap',
} as const;

interface Resolver {
  placeName: (placeId: string) => string;
  placeColor: (placeId: string) => string;
  stopPlaceId: (stopId: string) => string | null;
  stopDayLabel: (stopId: string) => string;
  dayLabel: (dayId: string) => string;
}

function makeResolver(detail: PlanDetail, extraPlaces: Place[], t: PlanTranslate): Resolver {
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
      return dayId ? t('plan.change.day', { day: dayNumber.get(dayId) ?? '?' }) : t('plan.change.plan');
    },
    dayLabel: (dayId) => t('plan.change.day', { day: dayNumber.get(dayId) ?? '?' }),
  };
}

function Dot({ color }: { color: string }) {
  return <span className="dot" style={{ background: color }} />;
}

function opRow(
  op: ChangeOp,
  i: number,
  r: Resolver,
  t: PlanTranslate,
  formatDate: (iso: string, options?: Intl.DateTimeFormatOptions) => string,
) {
  switch (op.op) {
    case 'add_stop':
      return (
        <Chg key={i} verb="add">
          <Dot color={r.placeColor(op.placeId)} />
          <span className="place">{r.placeName(op.placeId)}</span> <span className="arrow">→</span>{' '}
          {r.dayLabel(op.dayId)}, {t('plan.change.slot', { slot: Math.ceil(op.seq) })}
        </Chg>
      );
    case 'add_place_stop':
      // A place that doesn't exist yet — resolve from the draft, not the catalog.
      return (
        <Chg key={i} verb="add">
          <Dot color={PLACE_KIND_COLOR[op.draft.kind]} />
          <span className="place">{op.draft.name}</span>{' '}
          <span className="from">
            ({t('plan.change.newPlace', { city: op.draft.city })}
            {op.draft.lat != null && op.draft.lng != null ? ` · ${t('plan.change.pinned')}` : ''})
          </span>{' '}
          <span className="arrow">→</span> {r.dayLabel(op.dayId)}, {t('plan.change.slot', { slot: Math.ceil(op.seq) })}
        </Chg>
      );
    case 'remove_stop': {
      const placeId = r.stopPlaceId(op.stopId);
      return (
        <Chg key={i} verb="drop">
          {placeId && <Dot color={r.placeColor(placeId)} />}
          <span className="place">{placeId ? r.placeName(placeId) : op.stopId}</span>{' '}
          <span className="from">{t('plan.change.fromDay', { day: r.stopDayLabel(op.stopId) })}</span>
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
          {r.dayLabel(op.toDayId)}, {t('plan.change.slot', { slot: Math.ceil(op.seq) })}
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
          <span className="place">{t('plan.change.newDay')}</span> —{' '}
          {formatDate(op.date, { weekday: 'short', month: 'short', day: 'numeric' })} · {op.cityHint}
        </Chg>
      );
    case 'remove_day':
      return (
        <Chg key={i} verb="drop">
          <span className="place">{t('plan.change.dayRemoved', { day: r.dayLabel(op.dayId) })}</span>
        </Chg>
      );
  }
}

function Chg({ verb, children }: { verb: Verb; children: React.ReactNode }) {
  const { t } = useI18n();
  return (
    <div className="chg">
      <span className={`verb ${verb}`}>{t(VERB_KEY[verb])}</span>
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
  const { t, formatDate } = useI18n();
  const r = makeResolver(detail, extraPlaces ?? [], t);
  return (
    <div className={`changes${className ? ` ${className}` : ''}`}>
      {ops.map((op, i) => opRow(op, i, r, t, formatDate))}
    </div>
  );
}
