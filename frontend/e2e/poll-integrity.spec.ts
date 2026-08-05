import { test, expect } from '@playwright/test';
import { ApiError, MockApiClient } from '../src/api/mock/MockApiClient';

const TRIP = 't-japan26';

/** Model-level governance tests run in the existing Playwright CI harness. */
test('a tied top vote closes with no decision', async () => {
  const api = new MockApiClient();
  // Dinner starts 2 / 1 / 1. Kaiyu voting Uobei makes the top vote 2 / 2.
  await api.vote('t-japan26', 'poll-dinner', ['opt-uobei']);

  const poll = await api.closePoll('t-japan26', 'poll-dinner');

  expect(poll.status).toBe('failed');
  expect(poll.resolutionNote).toContain('tied result');
});

test('a tied plan-change poll never applies its structural proposal', async () => {
  const api = new MockApiClient();
  const before = await api.getCurrentPlan(TRIP);
  // The fixture is 2 Adopt / 1 Keep. Cast one fixture member's Keep vote, then
  // restore the leader identity that is allowed to close the poll.
  const session = api as unknown as { me: string };
  session.me = 'u-ann';
  await api.vote('t-japan26', 'poll-splitd6', ['opt-keep']);
  session.me = 'u-kaiyu';

  const poll = await api.closePoll('t-japan26', 'poll-splitd6');
  const after = await api.getCurrentPlan(TRIP);
  const proposal = (await api.listProposals(TRIP)).find((item) => item.id === 'prop-split-d6');

  expect(poll.status).toBe('failed');
  expect(poll.resolutionNote).toContain('tied result');
  expect(proposal?.status).toBe('pending');
  expect(after.plan.id).toBe(before.plan.id);
  expect(after.plan.version).toBe(before.plan.version);
});

test('a stale winning plan proposal closes failed without changing the plan', async () => {
  const api = new MockApiClient();
  const before = await api.getCurrentPlan(TRIP);

  // A leader fast-path proposal safely advances v3 to v4 first.
  await api.createProposal(TRIP, {
    title: 'Add a spare planning day',
    rationale: 'Advance the live plan to exercise optimistic locking.',
    changeSet: {
      basePlanVersion: before.plan.version,
      ops: [{ op: 'add_day', date: '2026-11-21', cityHint: 'Tokyo' }],
    },
    route: 'leader_approval',
  });
  const advanced = await api.getCurrentPlan(TRIP);
  expect(advanced.plan.version).toBe(before.plan.version + 1);

  // The fixture poll still wraps prop-split-d6 against v3; Adopt leads 2–1.
  const poll = await api.closePoll('t-japan26', 'poll-splitd6');
  const after = await api.getCurrentPlan(TRIP);
  const proposal = (await api.listProposals(TRIP)).find((item) => item.id === 'prop-split-d6');

  expect(poll.status).toBe('failed');
  expect(poll.resolutionNote).toContain('outdated plan');
  expect(proposal?.status).toBe('stale');
  expect(after.plan.id).toBe(advanced.plan.id);
  expect(after.plan.version).toBe(advanced.plan.version);
});

test('direct leader approval rejects a stale proposal with conflict', async () => {
  const api = new MockApiClient();
  const before = await api.getCurrentPlan(TRIP);
  await api.createProposal(TRIP, {
    title: 'Advance before direct approval',
    rationale: 'Create a newer live plan.',
    changeSet: {
      basePlanVersion: before.plan.version,
      ops: [{ op: 'add_day', date: '2026-11-21', cityHint: 'Tokyo' }],
    },
    route: 'leader_approval',
  });
  const advanced = await api.getCurrentPlan(TRIP);

  await expect(api.approveProposal('t-japan26', 'prop-split-d6')).rejects.toMatchObject<ApiError>({ status: 409 });
  const after = await api.getCurrentPlan(TRIP);
  const proposal = (await api.listProposals(TRIP)).find((item) => item.id === 'prop-split-d6');

  expect(proposal?.status).toBe('stale');
  expect(after.plan.id).toBe(advanced.plan.id);
  expect(after.plan.version).toBe(advanced.plan.version);
});
