import { expect, test } from '@playwright/test';
import type { CreatePollInput } from '../src/api/client';
import { ApiError, MockApiClient, computeLedger } from '../src/api/mock/MockApiClient';
import type {
  Candidate,
  ChangeOp,
  Day,
  Edit,
  Notice,
  Plan,
  PlanDetail,
  Poll,
  Proposal,
  Stop,
  Trip,
} from '../src/api/types';

const JAPAN = 't-japan26';
const OTHER_TRIP = 't-aegean27';

function historyEdit(index: number, status: Edit['status'] = 'applied'): Edit {
  return {
    id: `boundary-edit-${index}`,
    tripId: JAPAN,
    entity: 'trip',
    entityId: JAPAN,
    field: 'status',
    oldValue: 'dreaming',
    newValue: 'planning',
    author: 'u-kai',
    source: { via: 'web' },
    status,
    createdAt: '2026-08-06T10:00:00Z',
    revertedBy: status === 'reverted' ? 'u-kai' : null,
    revertedAt: status === 'reverted' ? '2026-08-06T11:00:00Z' : null,
    revertEditId: status === 'reverted' ? 'boundary-compensation' : null,
    revertsEditId: null,
  };
}

const privatePlace = {
  name: 'Aegean Secret Marina',
  kind: 'sight' as const,
  city: 'Naxos',
  address: 'Private harbour',
  website: null,
  phone: null,
  openingHours: [],
  photoUrls: [],
  guide: null,
};

async function otherPlanWithStop(): Promise<{ api: MockApiClient; detail: PlanDetail }> {
  const api = new MockApiClient();
  const candidate = await api.addCandidate(OTHER_TRIP, {
    sourcePlaceId: null,
    place: privatePlace,
    pitch: 'Only visible to this trip',
    tags: [],
  });
  const initial = await api.initializePlan(OTHER_TRIP, { anchorPlaceId: candidate.place.id });
  await api.createProposal(OTHER_TRIP, {
    title: 'Add the private marina',
    rationale: 'Make this saved place part of the itinerary',
    route: 'leader_approval',
    changeSet: {
      basePlanVersion: initial.plan.version,
      ops: [
        {
          op: 'add_stop',
          dayId: initial.days[0].id,
          placeId: candidate.place.id,
          seq: 1,
          stopKind: 'visit',
        },
      ],
    },
  });
  return { api, detail: await api.getCurrentPlan(OTHER_TRIP) };
}

/** Model-level checks for the trip-partition boundary frozen in OpenAPI. */
test('trip-owned child ids cannot be used through another trip route', async () => {
  const api = new MockApiClient();

  await expect(api.setCandidateStatus(OTHER_TRIP, 'c-ghibli', 'rejected')).rejects.toMatchObject<ApiError>({
    status: 404,
  });
  await expect(
    api.updateExpense(OTHER_TRIP, 'e-gracery', { note: 'cross-trip rewrite' }),
  ).rejects.toMatchObject<ApiError>({ status: 404 });
  await expect(api.getComments(OTHER_TRIP, 'th-onsen')).rejects.toMatchObject<ApiError>({ status: 404 });

  const candidate = (await api.listCandidates(JAPAN)).find((item) => item.id === 'c-ghibli');
  const expense = (await api.getLedger(JAPAN)).expenses.find((item) => item.id === 'e-gracery');
  expect(candidate?.status).toBe('shortlisted');
  expect(expense?.note).not.toBe('cross-trip rewrite');
});

test('content revert cannot mutate a day retained only in a historical plan', async () => {
  const api = new MockApiClient();
  const before = await api.getCurrentPlan(JAPAN);
  const target = before.days[0];
  const changedCity = `${target.cityHint} after edit`;
  await api.updateDay(JAPAN, target.id, { cityHint: changedCity });
  const edit = (await api.getHistory(JAPAN)).find(
    (item) => item.entity === 'day' && item.entityId === target.id && item.field === 'cityHint',
  );
  expect(edit).toBeDefined();

  // Model a newly published immutable version which no longer contains the
  // target day. The old row remains available only through the prior plan.
  const internals = api as unknown as { plans: Plan[]; days: Day[]; trips: Trip[] };
  const nextPlan: Plan = {
    id: 'plan-synthetic-current',
    tripId: JAPAN,
    version: before.plan.version + 1,
    createdFromProposalId: null,
    createdAt: '2026-08-06T12:00:00Z',
  };
  internals.plans.push(nextPlan);
  internals.days.push(
    ...before.days.filter((day) => day.id !== target.id).map((day) => ({ ...day, planId: nextPlan.id })),
  );
  const trip = internals.trips.find((item) => item.id === JAPAN);
  if (!trip || !edit) throw new Error('missing fixture trip or edit');
  trip.currentPlanId = nextPlan.id;

  await expect(api.revertEdit(JAPAN, edit.id)).rejects.toMatchObject<ApiError>({ status: 409 });
  expect(internals.days.find((day) => day.id === target.id && day.planId === before.plan.id)?.cityHint).toBe(
    changedCity,
  );
});

test('content history exposes only applied and reverted events', async () => {
  const api = new MockApiClient();
  const internals = api as unknown as { edits: Edit[] };
  internals.edits = [historyEdit(1), historyEdit(2, 'pending_review'), historyEdit(3, 'rejected')];

  await expect(api.getHistory(JAPAN)).resolves.toEqual([historyEdit(1)]);
  await expect(api.revertEdit(JAPAN, 'boundary-edit-2')).rejects.toMatchObject<ApiError>({ status: 404 });
  await expect(api.revertEdit(JAPAN, 'boundary-edit-3')).rejects.toMatchObject<ApiError>({ status: 404 });
});

test('content-history row and byte ceilings fail before mutation while completed reverts stay idempotent', async () => {
  const allowed = new MockApiClient();
  const allowedInternals = allowed as unknown as { edits: Edit[]; trips: Trip[] };
  allowedInternals.edits = Array.from({ length: 999 }, (_, index) => historyEdit(index));
  const allowedTrip = allowedInternals.trips.find((trip) => trip.id === JAPAN);
  if (!allowedTrip) throw new Error('missing fixture trip');
  allowedInternals.edits[0].newValue = allowedTrip.status;
  await allowed.revertEdit(JAPAN, allowedInternals.edits[0].id);
  expect(allowedInternals.edits).toHaveLength(1_000);

  const responseBoundary = new MockApiClient();
  const responseBoundaryInternals = responseBoundary as unknown as {
    edits: Edit[];
    trips: Trip[];
    me: string;
    nextId: number;
  };
  responseBoundaryInternals.edits = Array.from({ length: 999 }, (_, index) => historyEdit(index));
  const responseBoundaryTrip = responseBoundaryInternals.trips.find((trip) => trip.id === JAPAN);
  if (!responseBoundaryTrip) throw new Error('missing fixture trip');
  const responseBoundaryTarget = responseBoundaryInternals.edits[0];
  responseBoundaryTarget.newValue = responseBoundaryTrip.status;
  responseBoundaryInternals.edits[1].oldValue = '';
  const compensationId = `ed-${responseBoundaryInternals.nextId}`;
  const projectedTimestamp = '2026-08-06T12:00:00.000Z';
  const projectedReverted: Edit = {
    ...responseBoundaryTarget,
    status: 'reverted',
    revertedBy: responseBoundaryInternals.me,
    revertedAt: projectedTimestamp,
    revertEditId: compensationId,
  };
  const projectedCompensation: Edit = {
    id: compensationId,
    tripId: JAPAN,
    entity: responseBoundaryTarget.entity,
    entityId: responseBoundaryTarget.entityId,
    field: responseBoundaryTarget.field,
    oldValue: responseBoundaryTarget.newValue,
    newValue: responseBoundaryTarget.oldValue,
    author: responseBoundaryInternals.me,
    source: { via: 'web' },
    status: 'applied',
    createdAt: projectedTimestamp,
    revertedBy: null,
    revertedAt: null,
    revertEditId: null,
    revertsEditId: responseBoundaryTarget.id,
  };
  const projected = [...responseBoundaryInternals.edits.slice(1), projectedReverted, projectedCompensation];
  const encoder = new TextEncoder();
  const objectBytes = (edits: Edit[]) =>
    edits.reduce((total, edit) => total + encoder.encode(JSON.stringify(edit)).byteLength, 0);
  const byteLimit = 4 * 1_024 * 1_024;
  const projectedEnvelopeBytes = projected.length + 1;
  const targetObjectBytes = byteLimit - projectedEnvelopeBytes + 1;
  const paddingBytes = targetObjectBytes - objectBytes(projected);
  expect(paddingBytes).toBeGreaterThan(0);
  responseBoundaryInternals.edits[1].oldValue = 'x'.repeat(paddingBytes);
  expect(objectBytes(projected)).toBe(targetObjectBytes);
  await expect(responseBoundary.getHistory(JAPAN)).resolves.toHaveLength(999);
  const responseBoundaryStatus = responseBoundaryTrip.status;
  await expect(responseBoundary.revertEdit(JAPAN, responseBoundaryTarget.id)).rejects.toMatchObject<ApiError>({
    status: 409,
  });
  expect(responseBoundaryTrip.status).toBe(responseBoundaryStatus);
  expect(responseBoundaryInternals.edits).toHaveLength(999);

  const full = new MockApiClient();
  const fullInternals = full as unknown as { edits: Edit[]; trips: Trip[] };
  fullInternals.edits = Array.from({ length: 1_000 }, (_, index) => historyEdit(index));
  const fullTrip = fullInternals.trips.find((trip) => trip.id === JAPAN);
  if (!fullTrip) throw new Error('missing fixture trip');
  fullInternals.edits[0].newValue = fullTrip.status;
  const statusBefore = fullTrip.status;
  await expect(full.revertEdit(JAPAN, fullInternals.edits[0].id)).rejects.toMatchObject<ApiError>({ status: 409 });
  expect(fullTrip.status).toBe(statusBefore);
  expect(fullInternals.edits).toHaveLength(1_000);

  fullInternals.edits[0] = historyEdit(0, 'reverted');
  await expect(full.revertEdit(JAPAN, fullInternals.edits[0].id)).resolves.toBeUndefined();
  expect(fullInternals.edits).toHaveLength(1_000);

  fullInternals.edits.push(historyEdit(1_001));
  await expect(full.getHistory(JAPAN)).rejects.toMatchObject<ApiError>({ status: 409 });
  await expect(full.revertEdit(JAPAN, fullInternals.edits[0].id)).rejects.toMatchObject<ApiError>({ status: 409 });

  const oversized = new MockApiClient();
  const oversizedInternals = oversized as unknown as { edits: Edit[] };
  const large = historyEdit(0);
  large.oldValue = 'x'.repeat(4 * 1_024 * 1_024);
  oversizedInternals.edits = [large];
  await expect(oversized.getHistory(JAPAN)).rejects.toMatchObject<ApiError>({ status: 409 });
});

test('a rejected audience revert leaves both audience and completion stamps unchanged', async () => {
  const api = new MockApiClient();
  const internals = api as unknown as { edits: Edit[]; notices: Notice[] };
  const notice = internals.notices.find((item) => item.id === 'n-visa');
  if (!notice?.audience) throw new Error('missing audience-scoped notice fixture');
  const audienceEdit: Edit = {
    ...historyEdit(1_000),
    id: 'audience-revert-at-history-limit',
    entity: 'notice',
    entityId: notice.id,
    field: 'audience',
    oldValue: ['u-kaiyu'],
    newValue: [...notice.audience],
  };
  internals.edits = [...Array.from({ length: 999 }, (_, index) => historyEdit(index)), audienceEdit];
  const audienceBefore = [...notice.audience];
  const completionsBefore = notice.checklistItems.map((item) => [...item.doneBy]);

  await expect(api.revertEdit(JAPAN, audienceEdit.id)).rejects.toMatchObject<ApiError>({ status: 409 });
  expect(notice.audience).toEqual(audienceBefore);
  expect(notice.checklistItems.map((item) => item.doneBy)).toEqual(completionsBefore);
});

test('content revert cannot undo proposal-owned in-plan candidate state', async () => {
  const api = new MockApiClient();
  const internals = api as unknown as { edits: Edit[]; candidates: Candidate[] };
  const candidate = internals.candidates.find((item) => item.status === 'in_plan');
  if (!candidate) throw new Error('missing in-plan fixture candidate');
  const edit: Edit = {
    ...historyEdit(0),
    id: 'candidate-in-plan-edit',
    entity: 'candidate',
    entityId: candidate.id,
    field: 'status',
    oldValue: 'shortlisted',
    newValue: 'in_plan',
  };
  internals.edits = [edit];

  await expect(api.revertEdit(JAPAN, edit.id)).rejects.toMatchObject<ApiError>({ status: 409 });
  expect(candidate.status).toBe('in_plan');
});

test('place search and candidate sources cannot cross trip boundaries', async () => {
  const api = new MockApiClient();
  const privateCandidate = await api.addCandidate(OTHER_TRIP, {
    sourcePlaceId: null,
    place: privatePlace,
    pitch: 'Only visible to this trip',
    tags: [],
  });
  await expect(api.searchPlaces(OTHER_TRIP, 'Aegean Secret')).resolves.toEqual([]);

  const plan = await api.initializePlan(OTHER_TRIP, { anchorPlaceId: privateCandidate.place.id });
  await api.createProposal(OTHER_TRIP, {
    title: 'Add the private marina',
    rationale: 'Make this saved place part of the itinerary',
    route: 'leader_approval',
    changeSet: {
      basePlanVersion: plan.plan.version,
      ops: [
        {
          op: 'add_stop',
          dayId: plan.days[0].id,
          placeId: privateCandidate.place.id,
          seq: 1,
          stopKind: 'visit',
        },
      ],
    },
  });

  await expect(api.searchPlaces(OTHER_TRIP, 'Aegean Secret')).resolves.toContainEqual(privateCandidate.place);
  await expect(api.searchPlaces(JAPAN, 'Aegean Secret')).resolves.toEqual([]);
  await expect(
    api.addCandidate(JAPAN, {
      sourcePlaceId: privateCandidate.place.id,
      place: privatePlace,
      pitch: "Attempt to copy another trip's private snapshot",
      tags: [],
    }),
  ).rejects.toMatchObject<ApiError>({ status: 404 });
});

const foreignChangeOps: {
  name: string;
  build: (detail: PlanDetail) => ChangeOp;
}[] = [
  {
    name: 'add_stop day',
    build: () => ({ op: 'add_stop', dayId: 'd1', placeId: 'p-dotonbori', seq: 2, stopKind: 'visit' }),
  },
  {
    name: 'add_stop place',
    build: (detail) => ({
      op: 'add_stop',
      dayId: detail.days[0].id,
      placeId: 'p-gracery',
      seq: 2,
      stopKind: 'visit',
    }),
  },
  {
    name: 'add_place_stop day',
    build: () => ({
      op: 'add_place_stop',
      dayId: 'd1',
      seq: 2,
      stopKind: 'visit',
      draft: { name: 'Draft', kind: 'sight', city: 'Naxos', note: '', url: null, lat: null, lng: null },
    }),
  },
  { name: 'remove_stop stop', build: () => ({ op: 'remove_stop', stopId: 's-d1-hotel' }) },
  {
    name: 'move_stop stop',
    build: (detail) => ({ op: 'move_stop', stopId: 's-d1-hotel', toDayId: detail.days[0].id, seq: 1 }),
  },
  {
    name: 'move_stop destination day',
    build: (detail) => ({ op: 'move_stop', stopId: detail.stops[0].id, toDayId: 'd1', seq: 1 }),
  },
  {
    name: 'reorder day',
    build: (detail) => ({ op: 'reorder', dayId: 'd1', stopIdsInOrder: [detail.stops[0].id] }),
  },
  {
    name: 'reorder stop',
    build: (detail) => ({ op: 'reorder', dayId: detail.days[0].id, stopIdsInOrder: ['s-d1-hotel'] }),
  },
  {
    name: 'swap_place stop',
    build: () => ({ op: 'swap_place', stopId: 's-d1-hotel', newPlaceId: 'p-dotonbori' }),
  },
  {
    name: 'swap_place replacement',
    build: (detail) => ({ op: 'swap_place', stopId: detail.stops[0].id, newPlaceId: 'p-gracery' }),
  },
  { name: 'remove_day day', build: () => ({ op: 'remove_day', dayId: 'd1' }) },
];

for (const foreignOp of foreignChangeOps) {
  test(`structural proposals reject a foreign ${foreignOp.name} reference before writing`, async () => {
    const { api, detail } = await otherPlanWithStop();
    const beforeOther = await api.getCurrentPlan(OTHER_TRIP);
    const beforeJapan = await api.getCurrentPlan(JAPAN);
    const beforeProposals = await api.listProposals(OTHER_TRIP);

    await expect(
      api.createProposal(OTHER_TRIP, {
        title: 'Cross-trip structural attempt',
        rationale: 'This must be rejected before any state changes',
        route: 'leader_approval',
        changeSet: { basePlanVersion: detail.plan.version, ops: [foreignOp.build(detail)] },
      }),
    ).rejects.toMatchObject<ApiError>({ status: 404 });

    await expect(api.getCurrentPlan(OTHER_TRIP)).resolves.toEqual(beforeOther);
    await expect(api.getCurrentPlan(JAPAN)).resolves.toEqual(beforeJapan);
    await expect(api.listProposals(OTHER_TRIP)).resolves.toEqual(beforeProposals);
  });
}

test('proposal application revalidates every operation before changing the plan', async () => {
  const { api, detail: otherDetail } = await otherPlanWithStop();
  const before = await api.getCurrentPlan(JAPAN);
  const proposal = (api as unknown as { proposals: Proposal[] }).proposals.find((item) => item.id === 'prop-split-d6')!;
  proposal.changeSet = {
    basePlanVersion: before.plan.version,
    ops: [
      { op: 'remove_stop', stopId: 's-d5-nishiki' },
      { op: 'remove_stop', stopId: otherDetail.stops[0].id },
    ],
  };

  await expect(api.approveProposal(JAPAN, proposal.id)).rejects.toMatchObject<ApiError>({ status: 404 });
  await expect(api.getCurrentPlan(JAPAN)).resolves.toEqual(before);
  expect(proposal.status).toBe('pending');
});

test('public poll input cannot forge a plan-change poll or proposal link', async () => {
  const api = new MockApiClient();
  const before = await api.listPolls(OTHER_TRIP);
  const forged = {
    kind: 'plan_change',
    title: 'Adopt a foreign proposal',
    description: 'Untrusted request JSON must not create this poll kind',
    options: [{ label: 'Adopt', proposalId: 'prop-split-d6' }, { label: 'Keep current' }],
    closesAt: new Date(Date.now() + 86_400_000).toISOString(),
    allowMulti: false,
  } as unknown as CreatePollInput;

  await expect(api.createPoll(OTHER_TRIP, forged)).rejects.toMatchObject<ApiError>({ status: 400 });
  await expect(api.createPoll(OTHER_TRIP, { ...forged, kind: 'decision' })).rejects.toMatchObject<ApiError>({
    status: 400,
  });
  await expect(api.listPolls(OTHER_TRIP)).resolves.toEqual(before);
});

test('poll deadlines are UTC-only and stop late opening or ballot changes', async () => {
  const api = new MockApiClient();
  const future = new Date(Date.now() + 86_400_000).toISOString();
  const decision: CreatePollInput = {
    kind: 'decision',
    title: 'Deadline contract',
    description: '',
    options: [{ label: 'A' }, { label: 'B' }],
    closesAt: future.replace('Z', '+01:00'),
    allowMulti: false,
  };
  await expect(api.createPoll(JAPAN, decision)).rejects.toMatchObject<ApiError>({ status: 400 });

  const internals = api as unknown as { polls: Poll[] };
  const scheduled = internals.polls.find((poll) => poll.status === 'scheduled');
  const open = internals.polls.find((poll) => poll.status === 'open');
  if (!scheduled || !open) throw new Error('missing active poll fixtures');
  scheduled.closesAt = new Date(Date.now() - 1).toISOString();
  open.closesAt = new Date(Date.now() - 1).toISOString();

  await expect(api.openPoll(JAPAN, scheduled.id)).rejects.toMatchObject<ApiError>({ status: 409 });
  const replacement = open.options.find(
    (option) => !open.votes.some((vote) => vote.userId === 'u-kaiyu' && vote.optionId === option.id),
  );
  if (!replacement) throw new Error('missing replacement option');
  await expect(api.vote(JAPAN, open.id, [replacement.id])).rejects.toMatchObject<ApiError>({ status: 409 });
});

test('closing a plan-change poll resolves its proposal inside the poll trip', async () => {
  const api = new MockApiClient();
  const beforeJapan = await api.getCurrentPlan(JAPAN);
  const forgedPoll: Poll = {
    id: 'poll-cross-trip-proposal',
    tripId: OTHER_TRIP,
    createdBy: 'u-kaiyu',
    kind: 'plan_change',
    title: 'Foreign proposal pointer',
    description: 'Models a corrupt or legacy poll row',
    options: [
      { id: 'opt-adopt', label: 'Adopt', proposalId: 'prop-split-d6' },
      { id: 'opt-keep', label: 'Keep', proposalId: null },
    ],
    opensAt: null,
    closesAt: new Date(Date.now() + 86_400_000).toISOString(),
    quorum: 1,
    allowMulti: false,
    status: 'open',
    resolutionNote: null,
    votes: [{ userId: 'u-kaiyu', optionId: 'opt-adopt', at: new Date().toISOString() }],
  };
  (api as unknown as { polls: Poll[] }).polls.push(forgedPoll);

  await expect(api.closePoll(OTHER_TRIP, forgedPoll.id)).resolves.toMatchObject({ status: 'failed' });
  await expect(api.getCurrentPlan(JAPAN)).resolves.toEqual(beforeJapan);
});

test('expense corrections validate the complete merged row before mutation', async () => {
  const { api, detail: otherDetail } = await otherPlanWithStop();
  const before = (await api.getLedger(JAPAN)).expenses.find((expense) => expense.id === 'e-gracery');
  expect(before).toBeDefined();

  await expect(api.updateExpense(JAPAN, 'e-gracery', { paidBy: 'u-not-a-member' })).rejects.toMatchObject<ApiError>({
    status: 400,
  });
  await expect(
    api.updateExpense(JAPAN, 'e-gracery', {
      amount: 100,
      split: { kind: 'exact', participants: [{ userId: 'u-kaiyu', amount: 99 }] },
    }),
  ).rejects.toMatchObject<ApiError>({ status: 400 });
  await expect(
    api.updateExpense(JAPAN, 'e-gracery', { linkedStopId: otherDetail.stops[0].id }),
  ).rejects.toMatchObject<ApiError>({ status: 404 });

  const after = (await api.getLedger(JAPAN)).expenses.find((expense) => expense.id === 'e-gracery');
  expect(after).toEqual(before);
});

test('deleting an expense clears booking links only inside the route trip', async () => {
  const { api, detail } = await otherPlanWithStop();
  const otherStop = (api as unknown as { stops: Stop[] }).stops.find((stop) => stop.id === detail.stops[0].id)!;
  otherStop.booking = { ref: 'AEGEAN-1', url: null, cost: null, ledgerEntryId: 'e-gracery' };

  await api.deleteExpense(JAPAN, 'e-gracery');

  const japan = await api.getCurrentPlan(JAPAN);
  expect(japan.stops.find((stop) => stop.id === 's-d1-hotel')?.booking?.ledgerEntryId).toBeNull();
  expect((await api.getCurrentPlan(OTHER_TRIP)).stops[0].booking?.ledgerEntryId).toBe('e-gracery');
});

test('ledger simplification preserves each debtor and keeps referenced former members', async () => {
  const api = new MockApiClient();
  const trip = await api.getTrip(JAPAN);
  const members = ['debtor-a', 'debtor-b', 'creditor'].map((userId, index) => ({
    ...trip.members[index],
    userId,
  }));
  const expense = {
    id: 'expense-rounding',
    tripId: trip.id,
    paidBy: 'creditor',
    amount: 11,
    currency: trip.baseCurrency,
    fxRateToBase: 1,
    category: 'food' as const,
    split: {
      kind: 'exact' as const,
      participants: [
        { userId: 'debtor-a', amount: 9 },
        { userId: 'debtor-b', amount: 2 },
      ],
    },
    note: 'Shared meal',
    receiptPhotoUrl: null,
    linkedStopId: null,
    createdAt: '2026-08-06T10:00:00Z',
  };
  const view = computeLedger({ ...trip, members }, [expense], []);
  expect(view.suggestedTransfers).toEqual([
    { fromUser: 'debtor-a', toUser: 'creditor', amount: 9 },
    { fromUser: 'debtor-b', toUser: 'creditor', amount: 2 },
  ]);

  const withoutDebtorB = computeLedger(
    { ...trip, members: members.filter((member) => member.userId !== 'debtor-b') },
    [expense],
    [],
  );
  expect(withoutDebtorB.balances.some((balance) => balance.userId === 'debtor-b')).toBe(true);

  const halfUnit = computeLedger(
    { ...trip, members: members.slice(0, 2) },
    [
      {
        ...expense,
        id: 'expense-half-unit',
        paidBy: 'debtor-a',
        amount: 1,
        split: { kind: 'even', participantIds: ['debtor-a', 'debtor-b'] },
      },
    ],
    [],
  );
  expect(halfUnit.suggestedTransfers).toEqual([{ fromUser: 'debtor-b', toUser: 'debtor-a', amount: 1 }]);
});
