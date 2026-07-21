import type { Edit, Poll, Proposal, ReviewItem } from '../../types';

/**
 * Governance fixtures exercising every state the UI must render:
 * applied / pending / rejected / draft proposals, open + closed polls,
 * applied / reverted / pending_review edits, and the AI review queue.
 */

export const proposals: Proposal[] = [
  // Applied — this is what created plan v3 (leader's own edit, auto-approved)
  {
    id: 'prop-teamlab',
    tripId: 't-japan26',
    createdBy: 'u-kaiyu',
    source: { via: 'web' },
    title: 'Add teamLab Planets to Day 3',
    rationale: 'Ann\'s candidate won everyone over; the 16:00 slot catches dusk in the garden dome and Toyosu is 40 min from Asakusa.',
    changeSet: { basePlanVersion: 2, ops: [{ op: 'add_stop', dayId: 'd3', placeId: 'p-teamlab', seq: 3, stopKind: 'activity' }] },
    route: 'leader_approval',
    status: 'applied',
    decidedBy: { kind: 'leader', userId: 'u-kaiyu' },
    createdAt: '2026-07-09T15:30:00Z',
  },

  // Pending — routed to a poll, currently open (poll-splitd6)
  {
    id: 'prop-split-d6',
    tripId: 't-japan26',
    createdBy: 'u-makoto',
    source: { via: 'web' },
    title: 'Split Day 6: move Arashiyama to Day 5 afternoon',
    rationale: 'Day 6 runs at 87% of its window and sunset is ~16:45 — one slow lunch and we\'re walking the bamboo grove in the dark. Moving Arashiyama to Day 5 (dropping the Nishiki graze) gives both days room to breathe.',
    changeSet: {
      basePlanVersion: 3,
      ops: [
        { op: 'remove_stop', stopId: 's-d5-nishiki' },
        { op: 'move_stop', stopId: 's-d6-arashiyama', toDayId: 'd5', seq: 3 },
        { op: 'move_stop', stopId: 's-d6-yoshimura', toDayId: 'd5', seq: 4 },
      ],
    },
    route: 'poll',
    status: 'pending',
    decidedBy: { kind: 'poll', pollId: 'poll-splitd6' },
    createdAt: '2026-07-17T09:20:00Z',
  },

  // Rejected — feasibility engine said no, leader agreed
  {
    id: 'prop-usj',
    tripId: 't-japan26',
    createdBy: 'u-futaba',
    source: { via: 'web' },
    title: 'USJ speedrun on Day 7 before the flight',
    rationale: 'Gates at 08:00, Express Pass, leave by 14:00, straight to KIX. It\'s POSSIBLE.',
    changeSet: {
      basePlanVersion: 3,
      ops: [
        { op: 'remove_stop', stopId: 's-d7-osakacastle' },
        { op: 'remove_stop', stopId: 's-d7-kuromon' },
        { op: 'remove_stop', stopId: 's-d7-dotonbori' },
        { op: 'add_stop', dayId: 'd7', placeId: 'p-usj', seq: 1, stopKind: 'activity' },
      ],
    },
    route: 'leader_approval',
    status: 'rejected',
    decidedBy: { kind: 'leader', userId: 'u-makoto' },
    createdAt: '2026-07-14T20:41:00Z',
  },

  // Draft — AI-originated, sitting in Kaiyu's review queue (not yet published)
  {
    id: 'prop-kiyomizu-night',
    tripId: 't-japan26',
    createdBy: 'u-kaiyu',
    source: { via: 'token', tokenId: 'tok-claude', tokenName: 'claude' },
    title: 'Move Kiyomizu-dera to the evening for the autumn illumination',
    rationale: 'Kiyomizu-dera runs special night illuminations in mid-late November (typically 17:30–21:00). Swapping it to an evening slot converts Day 6\'s weakest hour into its highlight and relieves the morning crunch.',
    changeSet: { basePlanVersion: 3, ops: [{ op: 'reorder', dayId: 'd6', stopIdsInOrder: ['s-d6-fushimi', 's-d6-yoshimura', 's-d6-arashiyama', 's-d6-kiyomizu'] }] },
    route: 'leader_approval',
    status: 'draft',
    decidedBy: null,
    createdAt: '2026-07-20T14:12:00Z',
  },
];

export const polls: Poll[] = [
  // Open decision poll — dinner fight
  {
    id: 'poll-dinner',
    tripId: 't-japan26',
    createdBy: 'u-ryuji',
    kind: 'decision',
    title: 'Day 2 dinner in Shibuya',
    description: 'Winner gets us after the Scramble. Choose wisely.',
    options: [
      { id: 'opt-ichiran', label: 'Ichiran (ramen, solo booths)', proposalId: null },
      { id: 'opt-uobei', label: 'Uobei (bullet-train sushi, cheap)', proposalId: null },
      { id: 'opt-gyukatsu', label: 'Gyukatsu Motomura (beef cutlet)', proposalId: null },
    ],
    closesAt: '2026-07-25T12:00:00Z',
    quorum: 3,
    allowMulti: false,
    status: 'open',
    votes: [
      { userId: 'u-ryuji', optionId: 'opt-ichiran', at: '2026-07-15T10:00:00Z' },
      { userId: 'u-futaba', optionId: 'opt-ichiran', at: '2026-07-15T10:03:00Z' },
      { userId: 'u-ann', optionId: 'opt-gyukatsu', at: '2026-07-15T18:30:00Z' },
      { userId: 'u-yusuke', optionId: 'opt-uobei', at: '2026-07-16T07:12:00Z' },
    ],
  },

  // Open plan_change poll — wraps prop-split-d6
  {
    id: 'poll-splitd6',
    tripId: 't-japan26',
    createdBy: 'u-makoto',
    kind: 'plan_change',
    title: 'Restructure Day 6 (Makoto\'s proposal)',
    description: 'Adopt "Split Day 6: move Arashiyama to Day 5 afternoon"? See the diff preview for exactly what moves.',
    options: [
      { id: 'opt-adopt', label: 'Adopt the change', proposalId: 'prop-split-d6' },
      { id: 'opt-keep', label: 'Keep the current plan', proposalId: null },
    ],
    closesAt: '2026-07-28T12:00:00Z',
    quorum: 3,
    allowMulti: false,
    status: 'open',
    votes: [
      { userId: 'u-makoto', optionId: 'opt-adopt', at: '2026-07-17T09:25:00Z' },
      { userId: 'u-kaiyu', optionId: 'opt-adopt', at: '2026-07-17T11:40:00Z' },
      { userId: 'u-ryuji', optionId: 'opt-keep', at: '2026-07-18T08:02:00Z' },
    ],
  },

  // Closed & passed — how Hakone beat Nikkō
  {
    id: 'poll-onsen',
    tripId: 't-japan26',
    createdBy: 'u-kaiyu',
    kind: 'decision',
    title: 'Onsen night: Hakone or Nikkō?',
    description: 'One ryokan night on the way west. Hakone chains onto the Kyoto leg; Nikkō is a dead-end detour but less touristy.',
    options: [
      { id: 'opt-hakone', label: 'Hakone', proposalId: null },
      { id: 'opt-nikko', label: 'Nikkō', proposalId: null },
    ],
    closesAt: '2026-07-10T12:00:00Z',
    quorum: 3,
    allowMulti: false,
    status: 'passed',
    votes: [
      { userId: 'u-kaiyu', optionId: 'opt-hakone', at: '2026-07-08T09:00:00Z' },
      { userId: 'u-makoto', optionId: 'opt-hakone', at: '2026-07-08T09:30:00Z' },
      { userId: 'u-ann', optionId: 'opt-hakone', at: '2026-07-08T14:00:00Z' },
      { userId: 'u-ryuji', optionId: 'opt-hakone', at: '2026-07-09T10:00:00Z' },
      { userId: 'u-yusuke', optionId: 'opt-nikko', at: '2026-07-09T22:15:00Z' },
    ],
  },
];

export const edits: Edit[] = [
  { id: 'ed-1', tripId: 't-japan26', entity: 'stop', entityId: 's-d4-ryokan', field: 'booking', oldValue: null, newValue: { ref: 'ICHINOYU-118422' }, author: 'u-makoto', source: { via: 'web' }, status: 'applied', createdAt: '2026-07-12T13:05:00Z' },
  { id: 'ed-2', tripId: 't-japan26', entity: 'stop', entityId: 's-d2-shibuya', field: 'notes', oldValue: 'Crossing + Hachikō + shopping.', newValue: 'Crossing + Hachikō + shopping. Dinner nearby — winner of the open poll.', author: 'u-ann', source: { via: 'web' }, status: 'applied', createdAt: '2026-07-13T16:44:00Z' },
  { id: 'ed-3', tripId: 't-japan26', entity: 'stop', entityId: 's-d1-omoide', field: 'plannedArrival', oldValue: '19:00', newValue: '20:30', author: 'u-ryuji', source: { via: 'web' }, status: 'reverted', createdAt: '2026-07-14T11:20:00Z' },
  { id: 'ed-4', tripId: 't-japan26', entity: 'notice', entityId: 'n-money', field: 'body', oldValue: '(previous draft)', newValue: '(current text — added the ¥21,000 point-to-point total)', author: 'u-kaiyu', source: { via: 'web' }, status: 'applied', createdAt: '2026-07-16T19:55:00Z' },

  // Pending review — arrived via Kaiyu's "claude" token (the AI airlock)
  { id: 'ed-5', tripId: 't-japan26', entity: 'stop', entityId: 's-d6-fushimi', field: 'notes', oldValue: 'Dawn start beats the crowds.', newValue: 'Dawn start beats the crowds. Turn back at the Yotsutsuji viewpoint for the 90-min version — the full summit loop takes ~2 h.', author: 'u-kaiyu', source: { via: 'token', tokenId: 'tok-claude', tokenName: 'claude' }, status: 'pending_review', createdAt: '2026-07-20T14:10:00Z' },
  { id: 'ed-6', tripId: 't-japan26', entity: 'notice', entityId: 'n-connectivity', field: 'body', oldValue: '(current text)', newValue: '(adds an eSIM price comparison: Ubigi 10 GB ≈ $17, Airalo Moshi Moshi 10 GB ≈ $18, both cheaper than a second pocket wifi)', author: 'u-kaiyu', source: { via: 'token', tokenId: 'tok-claude', tokenName: 'claude' }, status: 'pending_review', createdAt: '2026-07-20T14:15:00Z' },
];

/** Kaiyu's review queue — everything the "claude" token drafted, awaiting approval. */
export const reviewItems: ReviewItem[] = [
  { id: 'rv-1', kind: 'edit', edit: edits.find((e) => e.id === 'ed-5')! },
  { id: 'rv-2', kind: 'edit', edit: edits.find((e) => e.id === 'ed-6')! },
  { id: 'rv-3', kind: 'proposal', proposal: proposals.find((p) => p.id === 'prop-kiyomizu-night')! },
];
