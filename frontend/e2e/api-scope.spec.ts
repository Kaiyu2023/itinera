import { expect, test } from '@playwright/test';
import type { CreatePollInput } from '../src/api/client';
import { ApiError, MockApiClient } from '../src/api/mock/MockApiClient';
import type { ChangeOp, PlanDetail, Poll, Proposal, Stop } from '../src/api/types';

const JAPAN = 't-japan26';
const OTHER_TRIP = 't-aegean27';

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
