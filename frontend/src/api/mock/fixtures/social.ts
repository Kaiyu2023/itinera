import type { Comment, ServiceIdentity, Thread } from '../../types';

export const threads: Thread[] = [
  {
    id: 'th-onsen',
    tripId: 't-japan26',
    anchor: { kind: 'stop', stopId: 's-d4-ryokan' },
    title: 'Onsen etiquette + the tattoo question',
    commentCount: 3,
    lastActivityAt: '2026-07-13T10:02:00Z',
  },
  {
    id: 'th-dinner',
    tripId: 't-japan26',
    anchor: { kind: 'poll', pollId: 'poll-dinner' },
    title: 'The great Shibuya dinner debate',
    commentCount: 3,
    lastActivityAt: '2026-07-16T07:15:00Z',
  },
  {
    id: 'th-flights',
    tripId: 't-japan26',
    anchor: { kind: 'trip' },
    title: 'Flight booking status',
    commentCount: 3,
    lastActivityAt: '2026-07-19T16:40:00Z',
  },
];

export const comments: Comment[] = [
  // th-onsen
  {
    id: 'cm-1',
    threadId: 'th-onsen',
    author: 'u-ann',
    body: 'Real question: are my tattoos going to be a problem at the ryokan?',
    createdAt: '2026-07-13T09:40:00Z',
    reactions: [],
  },
  {
    id: 'cm-2',
    threadId: 'th-onsen',
    author: 'u-makoto',
    body: 'Checked when I booked — Ichinoyu has a **private riverside bath** you can reserve at check-in, no restrictions there. Shared baths: cover or skip. I put the details in the "Ryokan crash course" notice.',
    createdAt: '2026-07-13T09:55:00Z',
    reactions: [{ emoji: '🙏', userIds: ['u-ann', 'u-kaiyu'] }],
  },
  {
    id: 'cm-3',
    threadId: 'th-onsen',
    author: 'u-ryuji',
    body: 'kaiseki at 18:30 *sharp*?? what happens at 18:31',
    createdAt: '2026-07-13T10:02:00Z',
    reactions: [{ emoji: '😂', userIds: ['u-futaba', 'u-ann'] }],
  },

  // th-dinner
  {
    id: 'cm-4',
    threadId: 'th-dinner',
    author: 'u-ryuji',
    body: "Ichiran is the correct answer and the booths mean I don't have to watch Yusuke eat sushi one grain of rice at a time.",
    createdAt: '2026-07-15T10:05:00Z',
    reactions: [{ emoji: '😂', userIds: ['u-futaba'] }],
  },
  {
    id: 'cm-5',
    threadId: 'th-dinner',
    author: 'u-ann',
    body: 'We are in Japan for SEVEN DAYS and you want chain ramen you can get at home. Motomura is ¥1,500 for beef that changes your life.',
    createdAt: '2026-07-15T18:32:00Z',
    reactions: [{ emoji: '💯', userIds: ['u-makoto'] }],
  },
  {
    id: 'cm-6',
    threadId: 'th-dinner',
    author: 'u-futaba',
    body: 'ichiran solo booths = zero social interaction while eating. 10/10. voted.',
    createdAt: '2026-07-16T07:15:00Z',
    reactions: [{ emoji: '😂', userIds: ['u-ryuji', 'u-ann'] }],
  },

  // th-flights
  {
    id: 'cm-7',
    threadId: 'th-flights',
    author: 'u-makoto',
    body: 'Status check: Kaiyu ✅, me ✅, Ann ✅ (same NH flight), Futaba ✅. Ryuji and Yusuke — the fare goes up after August.',
    createdAt: '2026-07-19T09:00:00Z',
    reactions: [],
  },
  {
    id: 'cm-8',
    threadId: 'th-flights',
    author: 'u-yusuke',
    body: 'I found a remarkable itinerary for ¥38,000: two layovers, 31 hours, arrives via Kuala Lumpur. The savings could fund an entire day of museum admissions.',
    createdAt: '2026-07-19T16:20:00Z',
    reactions: [{ emoji: '💀', userIds: ['u-ann', 'u-futaba', 'u-ryuji'] }],
  },
  {
    id: 'cm-9',
    threadId: 'th-flights',
    author: 'u-ann',
    body: "Yusuke. No. You will arrive as a fossil. Book the direct one, we'll sort the ledger out.",
    createdAt: '2026-07-19T16:40:00Z',
    reactions: [{ emoji: '😂', userIds: ['u-kaiyu'] }],
  },
];

/** Kaiyu's Cloudflare service mappings (visible only to their owner). */
export const serviceIdentities: ServiceIdentity[] = [
  {
    id: 'svc-claude',
    name: 'claude',
    clientIdHint: '41c0a12e',
    scopes: ['read', 'propose'],
    tripIds: ['trip-japan'],
    expiresAt: '2026-07-27T08:00:00Z',
    lastUsedAt: '2026-07-20T14:15:00Z',
    revokedAt: null,
    createdAt: '2026-07-20T08:00:00Z',
  },
  {
    id: 'svc-chatgpt',
    name: 'chatgpt-research',
    clientIdHint: '87b239d1',
    scopes: ['read'],
    tripIds: ['trip-japan'],
    expiresAt: '2026-07-06T09:00:00Z',
    lastUsedAt: '2026-07-05T20:11:00Z',
    revokedAt: null,
    createdAt: '2026-07-05T09:00:00Z',
  },
];
