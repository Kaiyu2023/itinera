import type { Candidate, Trip } from '../../types';

/**
 * A real, bookable 7-day Japan itinerary: Tokyo (3 nights) → Hakone ryokan
 * (1 night) → Kyoto (2 nights) → Osaka → fly out of KIX.
 * Nov 14–20 2026 is Sat–Fri, peak autumn foliage in Kyoto.
 */
export const trip: Trip = {
  id: 't-japan26',
  name: 'Japan, Autumn Leaves',
  coverPhotoUrl: '/photos/kiyomizu-main-hall.webp',
  accentColor: '#d97b4f', // momiji vermilion — this trip's, not the app's
  // Default kind names except lodging stops, which on this trip are all
  // check-ins — exercises the leader-customisable labels.
  stopKindLabels: { lodging: 'check-in' },
  status: 'planning',
  startDate: '2026-11-14',
  endDate: '2026-11-20',
  baseCurrency: 'USD',
  // Group soft cap of ¥180,000 / person (leader-set). The Ledger bar reads
  // total ÷ (amount × members); it never blocks a spend.
  softBudget: { amount: 180000, currency: 'JPY' },
  members: [
    { userId: 'u-kaiyu', role: 'leader', joinedAt: '2026-06-28T10:00:00Z' },
    { userId: 'u-makoto', role: 'leader', joinedAt: '2026-06-28T12:20:00Z' },
    { userId: 'u-ryuji', role: 'member', joinedAt: '2026-06-29T08:41:00Z' },
    { userId: 'u-ann', role: 'member', joinedAt: '2026-06-30T19:05:00Z' },
    { userId: 'u-yusuke', role: 'member', joinedAt: '2026-07-02T21:37:00Z' },
    { userId: 'u-futaba', role: 'member', joinedAt: '2026-07-03T02:11:00Z' },
  ],
  currentPlanId: 'plan-v3',
  createdAt: '2026-06-28T10:00:00Z',
};

/**
 * Two early-stage trips with no plan yet. They exist to exercise the trip
 * shelf and per-trip theming (each trip owns its accent; the app chrome
 * stays neutral). Dates are aspirational, hence status `dreaming`.
 */
export const dreamTrips: Trip[] = [
  {
    id: 't-aegean27',
    name: 'Aegean, Slow Boats',
    coverPhotoUrl: '/photos/oia-sunset.webp',
    accentColor: '#3e7fa8',
    stopKindLabels: null,
    status: 'dreaming',
    startDate: '2027-05-08',
    endDate: '2027-05-16',
    baseCurrency: 'EUR',
    members: [
      { userId: 'u-kaiyu', role: 'leader', joinedAt: '2026-07-10T18:00:00Z' },
      { userId: 'u-ann', role: 'member', joinedAt: '2026-07-10T18:24:00Z' },
      { userId: 'u-makoto', role: 'member', joinedAt: '2026-07-11T09:02:00Z' },
    ],
    currentPlanId: null,
    createdAt: '2026-07-10T18:00:00Z',
  },
  {
    id: 't-lofoten27',
    name: 'Lofoten, Midnight Sun',
    coverPhotoUrl: '/photos/reinebringen-lofoten.webp',
    accentColor: '#3f8f8a',
    stopKindLabels: null,
    status: 'dreaming',
    startDate: '2027-06-18',
    endDate: '2027-06-25',
    baseCurrency: 'EUR',
    members: [
      { userId: 'u-kaiyu', role: 'leader', joinedAt: '2026-07-15T20:30:00Z' },
      { userId: 'u-yusuke', role: 'member', joinedAt: '2026-07-16T07:45:00Z' },
      { userId: 'u-futaba', role: 'member', joinedAt: '2026-07-16T13:11:00Z' },
    ],
    currentPlanId: null,
    createdAt: '2026-07-15T20:30:00Z',
  },
];

/**
 * The shortlist. Everything already placed in the plan is `in_plan`;
 * `shortlisted` entries render as hollow dots on the candidates map layer.
 */
export const candidates: Candidate[] = [
  // Still competing for a slot
  {
    id: 'c-ghibli',
    tripId: 't-japan26',
    sourcePlaceId: 'p-ghibli',
    placeId: 'p-ghibli',
    proposedBy: 'u-ann',
    createdAt: '2026-07-03T09:12:00Z',
    pitch:
      'Tickets are basically a lottery, but if we get them this beats anything else in west Tokyo. Closed Tuesdays — would have to be Day 2 or 3.',
    tags: ['must-see', 'book-ahead'],
    status: 'shortlisted',
  },
  {
    id: 'c-todaiji',
    tripId: 't-japan26',
    sourcePlaceId: 'p-todaiji',
    placeId: 'p-todaiji',
    proposedBy: 'u-yusuke',
    createdAt: '2026-07-05T14:30:00Z',
    pitch:
      'The Great Buddha hall is one of the largest wooden buildings on earth. The deer are a bonus. Easy half-day from Kyoto on the Kintetsu line.',
    tags: ['day-trip'],
    status: 'shortlisted',
  },
  {
    id: 'c-usj',
    tripId: 't-japan26',
    sourcePlaceId: 'p-usj',
    placeId: 'p-usj',
    proposedBy: 'u-futaba',
    createdAt: '2026-07-06T18:00:00Z',
    pitch:
      'NINTENDO WORLD. Yes it eats a full day and ¥¥¥, yes we need Express Passes, no I will not be taking questions.',
    tags: ['splurge', 'full-day'],
    status: 'shortlisted',
  },
  {
    id: 'c-nijo',
    tripId: 't-japan26',
    sourcePlaceId: 'p-nijo',
    placeId: 'p-nijo',
    proposedBy: 'u-makoto',
    createdAt: '2026-07-08T10:02:00Z',
    pitch: 'Backup if Day 5 afternoon rains — nightingale floors, indoor, near the hostel.',
    tags: ['rainy-day'],
    status: 'shortlisted',
  },
  {
    id: 'c-tokichi',
    tripId: 't-japan26',
    sourcePlaceId: 'p-tokichi',
    placeId: 'p-tokichi',
    proposedBy: 'u-kaiyu',
    createdAt: '2026-07-18T11:47:00Z',
    pitch: 'Uji is the matcha capital and it sits right on the Nara line — pairs perfectly with the Tōdai-ji idea.',
    tags: ['pairs-with-nara'],
    status: 'shortlisted',
  },

  // Won a slot — in the current plan
  {
    id: 'c-teamlab',
    tripId: 't-japan26',
    sourcePlaceId: 'p-teamlab',
    placeId: 'p-teamlab',
    proposedBy: 'u-ann',
    createdAt: '2026-06-29T10:00:00Z',
    pitch: 'The water one in Toyosu. Wear shorts, book the 16:00 slot for dusk in the garden dome.',
    tags: ['book-ahead'],
    status: 'in_plan',
  },
  {
    id: 'c-fushimi',
    tripId: 't-japan26',
    sourcePlaceId: 'p-fushimi',
    placeId: 'p-fushimi',
    proposedBy: 'u-kaiyu',
    createdAt: '2026-06-29T10:05:00Z',
    pitch: 'Ten thousand torii gates. Non-negotiable. We go at dawn or we queue with everyone else.',
    tags: ['must-see', 'early-start'],
    status: 'in_plan',
  },
  {
    id: 'c-owakudani',
    tripId: 't-japan26',
    sourcePlaceId: 'p-owakudani',
    placeId: 'p-owakudani',
    proposedBy: 'u-ryuji',
    createdAt: '2026-07-01T16:22:00Z',
    pitch:
      "Volcanic valley, ropeway over the steam vents, black eggs that add 7 years to your life. Fuji view if it's clear.",
    tags: ['weather-dependent'],
    status: 'in_plan',
  },
  {
    id: 'c-ichinoyu',
    tripId: 't-japan26',
    sourcePlaceId: 'p-ichinoyu',
    placeId: 'p-ichinoyu',
    proposedBy: 'u-makoto',
    createdAt: '2026-07-01T16:30:00Z',
    pitch:
      'Operating since 1630, riverside baths, kaiseki dinner, and it will not bankrupt us like the famous Gōra places. One proper ryokan night.',
    tags: ['splurge', 'booked'],
    status: 'in_plan',
  },

  // Voted off
  {
    id: 'c-samurai',
    tripId: 't-japan26',
    sourcePlaceId: 'p-samurai',
    placeId: 'p-samurai',
    proposedBy: 'u-ryuji',
    createdAt: '2026-07-02T22:00:00Z',
    pitch: "Neon samurai dinner show in Kabukichō!! It's right next to our hotel!!",
    tags: [],
    status: 'rejected',
  },
];
