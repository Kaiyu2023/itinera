import { expect, test } from '@playwright/test';
import { ApiError, MockApiClient } from '../src/api/mock/MockApiClient';

const JAPAN = 't-japan26';
const OTHER_TRIP = 't-aegean27';

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
  const place = {
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
  const privateCandidate = await api.addCandidate(OTHER_TRIP, {
    sourcePlaceId: null,
    place,
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
      place,
      pitch: "Attempt to copy another trip's private snapshot",
      tags: [],
    }),
  ).rejects.toMatchObject<ApiError>({ status: 404 });
});
