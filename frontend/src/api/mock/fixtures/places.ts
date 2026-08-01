import type { Place, PlaceActivityIdea, PlaceGuide, PlaceKind } from '../../types';

/**
 * Global place catalog for the fixture trip (Tokyo → Hakone → Kyoto → Osaka).
 * Coordinates are real-world; ratings/hours are plausible mock data, cached
 * exactly as a PlaceCatalog adapter would cache them.
 */

interface PlaceSeed {
  id: string;
  name: string;
  kind: PlaceKind;
  lat: number;
  lng: number;
  city: string;
  adminArea: string;
  address: string;
  rating?: number;
  priceLevel?: number;
  website?: string;
  hours?: string[];
  guide?: PlaceGuide;
  /** Repo-hosted WebPs (see public/ATTRIBUTIONS.md) — R2 keys in prod. */
  photos?: string[];
}

function activity(title: string, details?: string): PlaceActivityIdea {
  return details ? { title, details } : { title };
}

/**
 * Editorial guides for every place used by the live plan or current shortlist.
 * Search-only catalog entries intentionally exercise the null-guide fallback.
 */
const PLACE_GUIDES: Record<string, PlaceGuide> = {
  'p-haneda': {
    summary: 'A city-side airport with fast rail links into central Tokyo.',
    intro:
      "Haneda is Tokyo's most convenient international gateway, combining compact terminals with direct rail access to the city.",
    activityIdeas: [
      activity(
        'Clear arrivals and collect luggage',
        'Use the Visit Japan Web lanes, then follow the colour-coded signs to baggage claim and customs.',
      ),
      activity(
        'Browse the terminal food halls',
        'The public areas have ramen, sushi and casual cafés that work well while the group regathers.',
      ),
    ],
    practicalTips: ['Keep the Visit Japan Web QR code ready', 'Confirm the terminal before following rail signs'],
  },
  'p-gracery': {
    summary: "A central Shinjuku base beneath Kabukicho's landmark Godzilla head.",
    intro:
      'Hotel Gracery sits in the heart of Kabukicho, useful for late Shinjuku evenings and well connected to the rest of Tokyo.',
    activityIdeas: [
      activity(
        'See the Godzilla head',
        'Look up from the hotel entrance on Central Road; scheduled sound and light effects run at selected times.',
      ),
      activity(
        'Walk through Kabukicho after dark',
        'Keep to the busy main streets for neon, arcades and late-night food without committing to a long route.',
      ),
    ],
    practicalTips: [
      'Check current guest access rules for the Godzilla terrace',
      'Allow time to walk from the main Shinjuku platforms',
    ],
  },
  'p-shibuya': {
    summary: "Tokyo's famous multi-direction crossing and a compact cluster of city icons.",
    intro:
      'Shibuya Scramble is the centrepiece of a dense neighbourhood of viewpoints, shopping, food and street-level Tokyo energy.',
    activityIdeas: [
      activity(
        'Cross with the crowd',
        'Wait through one light cycle, then cross diagonally when every direction opens at once. The centre is best for the full effect.',
      ),
      activity('Meet Hachiko'),
      activity(
        'Find an elevated view of the crossing',
        'Try a nearby café or observation deck to see the alternating surge and empty intersection from above.',
      ),
      activity(
        'Browse Center-gai for shops and snacks',
        'The pedestrian lanes opposite the station pack fashion, arcades and quick food into an easy short loop.',
      ),
    ],
    practicalTips: [
      'Choose a fixed meeting point before entering the crowd',
      'Expect the station exits to be confusing',
    ],
  },
  'p-omoide': {
    summary: 'A pair of lantern-lit alleys packed with tiny yakitori counters.',
    intro:
      'Omoide Yokocho preserves the close-quarters feel of old Shinjuku in a maze of smoke, grills and intimate counter seats.',
    activityIdeas: [
      activity(
        'Try a few yakitori skewers',
        'Order a small first round—each counter has its own cuts, seasoning and minimum-order customs.',
      ),
      activity(
        'Compare stalls across both alleys',
        'A quick walk through the parallel lanes makes it easier to spot open seats that suit the group.',
      ),
    ],
    practicalTips: ['Many stalls seat only a handful of people', 'Carry cash and split a large group'],
  },
  'p-sensoji': {
    summary: "Asakusa's landmark temple, approached through Kaminarimon and Nakamise.",
    intro:
      'Senso-ji is a historic Buddhist temple at the centre of Asakusa, with a ceremonial gate, shopping street and spacious grounds.',
    activityIdeas: [
      activity(
        'Enter through Kaminarimon',
        'Pause beneath the giant lantern, then look back at its carved base before continuing into Nakamise.',
      ),
      activity(
        'Visit Nakamise and the main hall',
        'Follow the shopping street to the incense burner, purification fountain and temple hall in one natural route.',
      ),
    ],
    practicalTips: ['Arrive early for quieter grounds', 'The grounds stay accessible longer than the main hall'],
  },
  'p-teamlab': {
    summary: 'An immersive digital-art route through water, light and garden rooms.',
    intro:
      'teamLab Planets is a timed, barefoot experience where visitors move through large-scale responsive installations and shallow water.',
    activityIdeas: [
      activity(
        'Walk the water installations',
        'You will go barefoot through knee-deep water, so wear clothes that roll up easily and use the provided lockers.',
      ),
      activity(
        'Explore the mirrored light and garden rooms',
        'Move slowly and look behind you—the responsive rooms change as people pass through them.',
      ),
    ],
    practicalTips: ['Book a timed entry in advance', 'Wear clothes that can be rolled above the knee'],
  },
  'p-tsukiji': {
    summary: 'A lively outer market for a breakfast crawl and specialist food shops.',
    intro:
      'Tsukiji Outer Market remains a dense network of seafood counters, produce shops and small kitchens even after the wholesale market moved.',
    activityIdeas: [
      activity(
        'Build a seafood breakfast crawl',
        'Share small portions from several counters instead of choosing one full meal, leaving room for seasonal specialties.',
      ),
      activity(
        'Browse tea, knife and pantry shops',
        'The side lanes mix restaurant suppliers with visitor-friendly shops for tea, cookware and dried goods.',
      ),
    ],
    practicalTips: ['Go in the morning before stalls sell out', 'Many businesses close on Sundays or Wednesdays'],
  },
  'p-meiji': {
    summary: 'A quiet forested Shinto shrine beside the bustle of Harajuku.',
    intro:
      "Meiji Jingu is reached by a long wooded approach that creates a calm transition from Harajuku to the shrine's main sanctuary.",
    activityIdeas: [
      activity(
        'Walk beneath the large torii gates',
        'The broad gravel approach is part of the experience: allow about ten quiet minutes from the Harajuku entrance to the shrine complex.',
      ),
      activity(
        'Visit the sanctuary and sake-barrel display',
        'At the main sanctuary, observe the purification ritual and bowing etiquette; the decorated barrels sit along the return approach.',
      ),
    ],
    practicalTips: ['Opening times follow daylight', 'Allow time for the walk from the entrance'],
  },
  'p-ghibli': {
    summary: "A small, imaginative museum devoted to Studio Ghibli's craft and worlds.",
    intro:
      'The Ghibli Museum in Mitaka combines animation exhibits, playful architecture and spaces designed to be explored rather than rushed.',
    activityIdeas: [
      activity(
        'Explore the animation exhibits',
        'The rooms explain movement, colour and hand-drawn filmmaking through working displays rather than a fixed linear route.',
      ),
      activity('Watch the museum-only short film and visit the rooftop'),
    ],
    practicalTips: ['Tickets are advance-only and date-specific', 'The museum is closed on Tuesdays'],
  },
  'p-gyozalou': {
    summary: 'A casual Harajuku counter for inexpensive pan-fried and steamed gyoza.',
    intro:
      'Harajuku Gyoza Lou is a compact, fast-moving dumpling stop that works well as a simple lunch between neighbourhood walks.',
    activityIdeas: [
      activity(
        'Compare fried and steamed gyoza',
        'Order one plate of each for contrasting crisp and soft wrappers; the compact menu makes sharing straightforward.',
      ),
      activity(
        'Watch the open kitchen from the counter',
        'Counter seats turn a quick meal into part of the experience as dumplings are folded and cooked in front of you.',
      ),
    ],
    practicalTips: ['Expect a queue around lunch', 'Small groups are seated more easily and cash is useful'],
  },
  'p-yumoto': {
    summary: "Hakone's rail gateway and the main transfer point for the mountain loop.",
    intro:
      'Hakone-Yumoto Station is where most visitors switch from intercity trains to local rail and bus services deeper into Hakone.',
    activityIdeas: [
      activity(
        'Store day bags in a locker',
        'Use a station locker or luggage service before starting the loop so transfers stay manageable.',
      ),
      activity(
        'Transfer to the bus or mountain railway',
        'Check the live service board first; weather can make one direction around the Hakone loop easier than the other.',
      ),
    ],
    practicalTips: ['Lockers can fill on busy mornings', 'Check the last return connection before setting out'],
  },
  'p-hakoneshrine': {
    summary: 'A cedar-shaded lakeside shrine known for its torii at the edge of Lake Ashi.',
    intro:
      "Hakone Shrine climbs through forest above Lake Ashi, while its lakeside torii creates the site's best-known view.",
    activityIdeas: [
      activity(
        'Climb to the main shrine',
        'The cedar-lined stairs lead to the quieter inner complex above the lake; damp weather can make them slippery.',
      ),
      activity(
        'Walk the shore and see the lakeside torii',
        'Follow the path downhill, but decide whether the often-long photo queue is worth the group’s time.',
      ),
    ],
    practicalTips: ['The lakeside photo queue can be long', 'Wear shoes suitable for damp steps'],
  },
  'p-owakudani': {
    summary: "A volcanic valley of steam vents, ropeway views and Hakone's black eggs.",
    intro:
      "Owakudani offers a close view of Hakone's active volcanic landscape from the ropeway and developed observation area.",
    activityIdeas: [
      activity(
        'Ride the ropeway over the valley',
        'The gondola gives the clearest view of the vents and sulphur fields when wind and volcanic conditions allow service.',
      ),
      activity(
        'Try a black egg and look for Mount Fuji',
        'The shell is darkened by the hot-spring minerals; the Fuji view is a weather-dependent bonus, not a guarantee.',
      ),
    ],
    practicalTips: ['Check ropeway and volcanic alerts that morning', 'Weather can stop service with little notice'],
  },
  'p-ichinoyu': {
    summary: 'A historic riverside ryokan for onsen, kaiseki and a slower Hakone evening.',
    intro:
      'Ichinoyu Honkan is a traditional inn in Tonosawa where the stay itself, from bathing to dinner, is the main experience.',
    activityIdeas: [
      activity(
        'Take an onsen bath',
        'Wash before entering, keep towels out of the water and check tattoo rules or private-bath availability in advance.',
      ),
      activity(
        'Settle in for kaiseki dinner',
        'Dinner is a paced sequence of small seasonal dishes, so confirm the seating time and arrive without needing to rush out.',
      ),
    ],
    practicalTips: ['Confirm the dinner seating at check-in', 'Ask early about reserving a private bath'],
  },
  'p-kyotostation': {
    summary: "Kyoto's huge transport hub, with food, shopping and useful city services.",
    intro:
      'Kyoto Station combines Shinkansen, local rail, subway and bus connections inside a large modern station complex.',
    activityIdeas: [
      activity(
        'Orient from the central concourse',
        'The station spans several levels and exits; identify the side you need before following the escalators.',
      ),
      activity(
        'Browse the food floors or arrange luggage forwarding',
        'Use the station as a practical reset for bento, gifts or sending larger bags ahead to the next hotel.',
      ),
    ],
    practicalTips: [
      'Allow extra transfer time between distant platforms',
      'Note the exit name before leaving the concourse',
    ],
  },
  'p-kyoto-hotel': {
    summary: 'A central Kyoto hostel base near downtown food and shopping.',
    intro:
      'Piece Hostel Sanjo is a practical central base for Kyoto evenings, with shared spaces and easy access to the downtown grid.',
    activityIdeas: [
      activity(
        'Drop bags before exploring downtown',
        'Confirm the pre-check-in storage window, then continue with only what is needed for the afternoon.',
      ),
      activity('Use the lounge to regroup'),
    ],
    practicalTips: ['Confirm luggage-drop and check-in hours', 'Keep noise low in shared sleeping areas'],
  },
  'p-nijo': {
    summary: 'A shogunal castle of palace rooms, gardens and famous nightingale floors.',
    intro:
      "Nijo Castle pairs the decorated Ninomaru Palace with broad gardens and fortifications from Kyoto's shogunal era.",
    activityIdeas: [
      activity(
        'Tour Ninomaru Palace',
        'Follow the one-way route through painted reception rooms and listen for the deliberately squeaking nightingale floors.',
      ),
      activity(
        'Walk the gardens and outer walls',
        'The grounds add seasonal planting, stone walls and a broader sense of the castle beyond its interiors.',
      ),
    ],
    practicalTips: ["Check the palace's last-entry time", 'It is a useful option when the weather turns wet'],
  },
  'p-gion': {
    summary: "Kyoto's historic entertainment district of machiya lanes and canals.",
    intro:
      'Gion is best explored as a slow walk through preserved streets around Hanamikoji, Shirakawa and the eastern edge of downtown.',
    activityIdeas: [
      activity(
        'Walk Hanamikoji and Shirakawa',
        'Link the machiya-lined street with the willow-lined canal for two distinct sides of historic Gion.',
      ),
      activity(
        'Continue toward Pontocho for dinner',
        'Cross back toward the river for a narrow restaurant alley with options ranging from casual counters to reservations.',
      ),
    ],
    practicalTips: [
      'Respect no-photography rules on private lanes',
      'Do not block doorways or follow working geiko and maiko',
    ],
  },
  'p-fushimi': {
    summary: 'A mountainside shrine whose paths pass through thousands of vermilion torii.',
    intro:
      'Fushimi Inari Taisha spreads from a busy lower shrine into a network of torii-lined paths climbing Mount Inari.',
    activityIdeas: [
      activity(
        'Visit the lower shrine',
        'See the main gates and prayer halls first; this compact circuit works even if the group skips the mountain climb.',
      ),
      activity(
        'Walk the torii tunnels to Yotsutsuji',
        'The climb to the viewpoint is the most rewarding middle-distance option and avoids committing to the full summit loop.',
      ),
    ],
    practicalTips: ['Start near dawn for quieter paths', 'The full summit loop is much longer than the lower circuit'],
  },
  'p-kiyomizu': {
    summary: 'A hillside temple with a broad wooden stage above eastern Kyoto.',
    intro:
      'Kiyomizu-dera combines a dramatic temple terrace with smaller halls and an atmospheric approach through Higashiyama.',
    activityIdeas: [
      activity(
        'Take in the view from the main stage',
        'Look across the wooded hillside toward Kyoto, then continue through the smaller halls below the terrace.',
      ),
      activity(
        'Walk down through Sannenzaka and Ninenzaka',
        'The preserved slopes turn the temple visit into a neighbourhood walk with ceramics, sweets and traditional façades.',
      ),
    ],
    practicalTips: ['Arrive early before the approach becomes crowded', 'The walk is steep in places'],
  },
  'p-arashiyama': {
    summary: "A short but atmospheric bamboo path at the foot of western Kyoto's hills.",
    intro:
      'Arashiyama Bamboo Grove is one part of a wider riverside district of temples, gardens and mountain scenery.',
    activityIdeas: [
      activity(
        'Walk the bamboo path',
        'The famous section is short, so enjoy the sound and light rather than treating it as the whole destination.',
      ),
      activity(
        'Continue to a garden or Togetsukyo Bridge',
        'Pair the grove with one nearby garden or the riverfront to make the journey across Kyoto worthwhile.',
      ),
    ],
    practicalTips: ['Go early for space and softer light', 'The bamboo section is shorter than many visitors expect'],
  },
  'p-nishiki': {
    summary: 'A narrow covered market lined with Kyoto snacks and pantry specialists.',
    intro:
      'Nishiki Market is a compact tasting route through local ingredients, prepared foods and long-running family shops.',
    activityIdeas: [
      activity(
        'Sample small market snacks',
        'Choose a few made-to-order bites and eat beside each stall instead of carrying food through the crowd.',
      ),
      activity(
        'Browse cookware, tea and pantry shops',
        'Look beyond ready-to-eat food for Kyoto knives, pickles, tea and ingredients that travel well.',
      ),
    ],
    practicalTips: ['Go before late-afternoon closures', 'Stand beside the stall to eat instead of blocking the crowd'],
  },
  'p-yoshimura': {
    summary: 'A riverside soba restaurant overlooking Arashiyama and Togetsukyo Bridge.',
    intro: 'Arashiyama Yoshimura turns a soba lunch into a scenic pause between the bamboo district and the river.',
    activityIdeas: [
      activity(
        'Order a hot or cold soba set',
        'Cold noodles highlight the buckwheat flavour; a hot broth is a comforting choice in cooler weather.',
      ),
      activity(
        'Walk the riverbank while waiting',
        'If there is a queue, use the time for a short loop near Togetsukyo instead of standing at the entrance.',
      ),
    ],
    practicalTips: ['Popular view seats go first', 'Leave schedule slack for the lunchtime queue'],
  },
  'p-tokichi': {
    summary: 'A historic Uji tea shop and cafe known for matcha desserts.',
    intro:
      'Nakamura Tokichi Honten combines a tea shop, cafe and garden in Uji, making it an easy food anchor for a Nara-line detour.',
    activityIdeas: [
      activity(
        'Try a matcha parfait or tea set',
        'A dessert is the most playful option, while a tea set gives more room to compare flavour and preparation.',
      ),
      activity(
        'Browse packaged tea and the garden',
        'The shop is useful for gifts, and the small garden offers a quieter pause around the café visit.',
      ),
    ],
    practicalTips: ['Join the cafe waiting list on arrival', 'Weekends can add a substantial queue'],
  },
  'p-todaiji': {
    summary: "Nara's monumental temple complex and the Great Buddha Hall.",
    intro:
      'Todai-ji anchors Nara Park with the vast Daibutsuden hall, a monumental bronze Buddha and worthwhile hillside buildings.',
    activityIdeas: [
      activity(
        'Enter the Great Buddha Hall',
        'Walk around the monumental bronze Buddha and look for the pillar opening traditionally linked with enlightenment.',
      ),
      activity(
        'Continue through Nara Park to Nigatsu-do',
        'The uphill extension is quieter than the main hall and ends with a broad view across Nara.',
      ),
    ],
    practicalTips: ['Allow time to walk from the station area', 'Keep maps and loose paper away from the deer'],
  },
  'p-osakacastle': {
    summary: 'A reconstructed castle keep surrounded by a large historic park.',
    intro:
      'Osaka Castle combines a museum inside the modern keep with broad grounds, moats and city views from the upper level.',
    activityIdeas: [
      activity(
        'Explore the park and moats',
        'The large grounds reveal the scale of the fortifications and offer the best exterior views of the keep.',
      ),
      activity(
        'Visit the exhibits and observation floor',
        'Inside is a modern history museum rather than a preserved castle interior, ending with a city panorama.',
      ),
    ],
    practicalTips: ['The park walk adds more time than the keep alone', 'Queues are busiest around midday'],
  },
  'p-dotonbori': {
    summary: "Osaka's neon canal district for signs, street food and people-watching.",
    intro:
      "Dotonbori concentrates Osaka's most recognisable signs, busy restaurant fronts and nightlife along a compact canal corridor.",
    activityIdeas: [
      activity('Photograph the Glico sign'),
      activity(
        'Try a street snack and walk the canal',
        'Share takoyaki or another small bite, then follow the water to take in the animated signs and side streets.',
      ),
    ],
    practicalTips: ['The signs are strongest after dark', 'Pick a meeting point before entering the densest crowds'],
  },
  'p-kuromon': {
    summary: 'A covered Osaka market for seafood, fruit and cooked-to-order bites.',
    intro:
      'Kuromon Ichiba is an easy grazing stop where fishmongers and produce shops serve small portions directly from their counters.',
    activityIdeas: [
      activity(
        'Try grilled seafood',
        'Compare clearly posted prices and share a skewer or shellfish portion before ordering a premium item by weight.',
      ),
      activity(
        'Browse fruit, wagyu and edible souvenirs',
        'The covered arcade mixes specialist produce with portable snacks, so it works for both tasting and gift shopping.',
      ),
    ],
    practicalTips: ['Visit earlier for the widest choice', 'Check prices before ordering premium seafood by weight'],
  },
  'p-usj': {
    summary: 'A full-day theme park built around major film and game worlds.',
    intro:
      'Universal Studios Japan combines headline rides with highly themed areas, including Super Nintendo World and the Wizarding World.',
    activityIdeas: [
      activity(
        'Explore Super Nintendo World',
        'Secure timed-area entry if required, then decide whether interactive Power-Up Band activities are worth the extra time.',
      ),
      activity(
        'Ride the headline attractions and see a show',
        'Prioritise a short must-do list around current wait times rather than trying to complete every land.',
      ),
    ],
    practicalTips: ['Buy a date-specific studio pass in advance', 'Check timed-area entry and Express Pass rules'],
  },
  'p-kix': {
    summary: "Osaka's offshore international airport and a hard-timed final connection.",
    intro:
      'Kansai International Airport sits on an artificial island, so the rail journey and terminal transfer need to be part of the departure plan.',
    activityIdeas: [
      activity(
        'Check in and clear security',
        'Confirm the terminal and leave recovery time for the bridge or rail connection before joining airline queues.',
      ),
      activity(
        'Have a final Kansai meal or buy gifts',
        'Keep this as a buffer activity after security, not a reason to delay the trip to the airport.',
      ),
    ],
    practicalTips: [
      'Build in recovery time for the long airport transfer',
      'Confirm the airline terminal before arrival',
    ],
  },
};

function jp(seed: PlaceSeed): Place {
  return {
    id: seed.id,
    name: seed.name,
    kind: seed.kind,
    lat: seed.lat,
    lng: seed.lng,
    tz: 'Asia/Tokyo',
    countryCode: 'JP',
    adminArea: seed.adminArea,
    city: seed.city,
    address: seed.address,
    externalRef: { provider: 'google', placeId: `mock-${seed.id}` },
    website: seed.website ?? null,
    phone: null,
    rating: seed.rating ?? null,
    priceLevel: seed.priceLevel ?? null,
    openingHours: seed.hours ? { weekdayText: seed.hours } : null,
    photoUrls: seed.photos ?? [],
    guide: seed.guide ?? PLACE_GUIDES[seed.id] ?? null,
  };
}

export const places: Place[] = [
  // --- Tokyo ---------------------------------------------------------------
  jp({
    id: 'p-haneda',
    name: 'Haneda Airport (HND)',
    kind: 'transport_hub',
    lat: 35.5494,
    lng: 139.7798,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: 'Ota City, Tokyo',
    photos: ['/photos/haneda-airport.webp', '/photos/haneda-keikyu-sign.webp'],
  }),
  jp({
    id: 'p-gracery',
    name: 'Hotel Gracery Shinjuku',
    kind: 'lodging',
    lat: 35.6952,
    lng: 139.7015,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-19-1 Kabukicho, Shinjuku',
    rating: 4.2,
    priceLevel: 3,
    website: 'https://shinjuku.gracery.com',
    photos: ['/photos/hotel-gracery-shinjuku.webp'],
  }),
  jp({
    id: 'p-shibuya',
    name: 'Shibuya Scramble Crossing',
    kind: 'sight',
    lat: 35.6595,
    lng: 139.7005,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: 'Shibuya City, Tokyo',
    rating: 4.4,
    photos: ['/photos/shibuya-crossing.webp', '/photos/shibuya-aerial.webp', '/photos/shibuya-2am.webp'],
  }),
  jp({
    id: 'p-omoide',
    name: 'Omoide Yokocho',
    kind: 'food',
    lat: 35.6938,
    lng: 139.6993,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-2 Nishishinjuku, Shinjuku',
    rating: 4.3,
    priceLevel: 2,
    hours: ['Most stalls: 17:00–24:00'],
    photos: ['/photos/omoide-yokocho.webp', '/photos/omoide-yokocho-entrance.webp'],
  }),
  jp({
    id: 'p-sensoji',
    name: 'Sensō-ji',
    kind: 'sight',
    lat: 35.7148,
    lng: 139.7967,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '2-3-1 Asakusa, Taito',
    rating: 4.5,
    hours: ['Grounds always open; main hall 06:00–17:00'],
    photos: [
      '/photos/sensoji-kaminarimon.webp',
      '/photos/sensoji-kaminarimon-night.webp',
      '/photos/sensoji-dragon-carving.webp',
    ],
  }),
  jp({
    id: 'p-teamlab',
    name: 'teamLab Planets',
    kind: 'activity',
    lat: 35.6494,
    lng: 139.7898,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '6-1-16 Toyosu, Koto',
    rating: 4.6,
    priceLevel: 3,
    website: 'https://www.teamlab.art/e/planets/',
    hours: ['09:00–22:00, timed entry'],
    photos: ['/photos/teamlab-planets.webp'],
  }),
  jp({
    id: 'p-tsukiji',
    name: 'Tsukiji Outer Market',
    kind: 'food',
    lat: 35.6654,
    lng: 139.7707,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '4 Chome Tsukiji, Chuo',
    rating: 4.4,
    priceLevel: 2,
    hours: ['Shops ~05:00–14:00', 'Many shops closed Sun & Wed — go on a weekday morning'],
    photos: ['/photos/tsukiji-outer-market.webp', '/photos/tsukiji-stall.webp'],
  }),
  jp({
    id: 'p-meiji',
    name: 'Meiji Jingū',
    kind: 'sight',
    lat: 35.6764,
    lng: 139.6993,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-1 Yoyogikamizonocho, Shibuya',
    rating: 4.6,
    hours: ['Sunrise to sunset'],
    photos: ['/photos/meiji-jingu-torii.webp', '/photos/meiji-torii-sun.webp'],
  }),
  jp({
    id: 'p-ichiran',
    name: 'Ichiran Shibuya',
    kind: 'food',
    lat: 35.6613,
    lng: 139.7004,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-22-7 Jinnan, Shibuya',
    rating: 4.3,
    priceLevel: 2,
    hours: ['24 hours'],
  }),
  jp({
    id: 'p-uobei',
    name: 'Uobei Sushi Shibuya',
    kind: 'food',
    lat: 35.658,
    lng: 139.6994,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '2-29-11 Dogenzaka, Shibuya',
    rating: 4.1,
    priceLevel: 1,
    hours: ['11:00–23:00'],
  }),
  jp({
    id: 'p-gyukatsu',
    name: 'Gyukatsu Motomura Shibuya',
    kind: 'food',
    lat: 35.657,
    lng: 139.7016,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '3-18-10 Shibuya',
    rating: 4.5,
    priceLevel: 2,
    hours: ['10:00–22:00'],
  }),
  jp({
    id: 'p-ghibli',
    name: 'Ghibli Museum',
    kind: 'activity',
    lat: 35.6962,
    lng: 139.5704,
    city: 'Mitaka',
    adminArea: 'Tokyo',
    address: '1-1-83 Shimorenjaku, Mitaka',
    rating: 4.5,
    priceLevel: 2,
    website: 'https://www.ghibli-museum.jp',
    hours: ['10:00–18:00, closed Tue', 'Tickets: lottery/advance only, on sale 10th of prior month 10:00 JST'],
    photos: ['/photos/ghibli-museum.webp'],
  }),
  jp({
    id: 'p-gyozalou',
    name: 'Harajuku Gyōza Lou',
    kind: 'food',
    lat: 35.669,
    lng: 139.7079,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '6-2-4 Jingumae, Shibuya',
    rating: 4.2,
    priceLevel: 1,
    hours: ['11:30–22:00 — expect a queue at noon'],
  }),
  jp({
    id: 'p-samurai',
    name: 'Samurai Restaurant (Kabukichō)',
    kind: 'activity',
    lat: 35.6949,
    lng: 139.702,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-7-7 Kabukicho, Shinjuku',
    rating: 3.9,
    priceLevel: 4,
    hours: ['Shows nightly, reservation required'],
  }),

  // --- Hakone ----------------------------------------------------------------
  jp({
    id: 'p-odawara',
    name: 'Odawara Station',
    kind: 'transport_hub',
    lat: 35.2564,
    lng: 139.1553,
    city: 'Odawara',
    adminArea: 'Kanagawa',
    address: 'Odawara, Kanagawa',
  }),
  jp({
    id: 'p-yumoto',
    name: 'Hakone-Yumoto Station',
    kind: 'transport_hub',
    lat: 35.2323,
    lng: 139.1069,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: 'Yumoto, Hakone',
    hours: ['Coin lockers by the main exit'],
    photos: ['/photos/hakone-yumoto-station.webp', '/photos/hakone-yumoto-escalator.webp'],
  }),
  jp({
    id: 'p-hakoneshrine',
    name: 'Hakone Shrine',
    kind: 'sight',
    lat: 35.2048,
    lng: 139.025,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: '80-1 Motohakone, Hakone',
    rating: 4.5,
    hours: ['Grounds always open'],
    photos: ['/photos/hakone-shrine.webp'],
  }),
  jp({
    id: 'p-owakudani',
    name: 'Ōwakudani (Hakone Ropeway)',
    kind: 'sight',
    lat: 35.2444,
    lng: 139.0194,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: 'Sengokuhara, Hakone',
    rating: 4.4,
    hours: ['Ropeway 09:00–17:00, weather-dependent'],
    photos: ['/photos/owakudani-fuji.webp'],
  }),
  jp({
    id: 'p-ichinoyu',
    name: 'Ichinoyu Honkan (ryokan, est. 1630)',
    kind: 'lodging',
    lat: 35.227,
    lng: 139.0968,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: '90 Tonosawa, Hakone',
    rating: 4.2,
    priceLevel: 3,
    website: 'https://www.ichinoyu.co.jp',
    hours: ['Check-in 15:00, dinner seatings 17:30 / 19:30'],
  }),

  // --- Kyoto -----------------------------------------------------------------
  jp({
    id: 'p-kyotostation',
    name: 'Kyoto Station',
    kind: 'transport_hub',
    lat: 34.9858,
    lng: 135.7588,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: 'Higashishiokoji, Shimogyo',
    photos: ['/photos/kyoto-station.webp'],
  }),
  jp({
    id: 'p-kyoto-hotel',
    name: 'Piece Hostel Sanjo',
    kind: 'lodging',
    lat: 35.0089,
    lng: 135.7622,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '531 Asakuracho, Nakagyo',
    rating: 4.4,
    priceLevel: 2,
  }),
  jp({
    id: 'p-nijo',
    name: 'Nijō Castle',
    kind: 'sight',
    lat: 35.0142,
    lng: 135.7481,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '541 Nijojocho, Nakagyo',
    rating: 4.4,
    priceLevel: 1,
    hours: ['08:45–17:00, last entry 16:00'],
    photos: ['/photos/nijo-castle.webp'],
  }),
  jp({
    id: 'p-gion',
    name: 'Gion (evening walk)',
    kind: 'sight',
    lat: 35.0037,
    lng: 135.778,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: 'Gion, Higashiyama',
    rating: 4.5,
    photos: ['/photos/gion-hanamikoji.webp'],
  }),
  jp({
    id: 'p-fushimi',
    name: 'Fushimi Inari Taisha',
    kind: 'sight',
    lat: 34.9671,
    lng: 135.7727,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '68 Fukakusa Yabunouchicho, Fushimi',
    rating: 4.6,
    hours: ['Always open — go at dawn to beat crowds'],
    photos: ['/photos/fushimi-inari-torii.webp', '/photos/fushimi-inari-tunnel.webp'],
  }),
  jp({
    id: 'p-kiyomizu',
    name: 'Kiyomizu-dera',
    kind: 'sight',
    lat: 34.9949,
    lng: 135.785,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '1-294 Kiyomizu, Higashiyama',
    rating: 4.5,
    priceLevel: 1,
    hours: ['06:00–18:00, last entry 17:30'],
    photos: ['/photos/kiyomizu-november.webp', '/photos/kiyomizu-stage.webp'],
  }),
  jp({
    id: 'p-arashiyama',
    name: 'Arashiyama Bamboo Grove',
    kind: 'sight',
    lat: 35.017,
    lng: 135.671,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: 'Sagaogurayama, Ukyo',
    rating: 4.4,
    hours: ['Always open'],
    photos: ['/photos/arashiyama-bamboo.webp', '/photos/arashiyama-bamboo-canopy.webp'],
  }),
  jp({
    id: 'p-nishiki',
    name: 'Nishiki Market',
    kind: 'food',
    lat: 35.005,
    lng: 135.7649,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: 'Nishikikoji-dori, Nakagyo',
    rating: 4.3,
    priceLevel: 2,
    hours: ['Most shops 10:00–17:00'],
    photos: ['/photos/nishiki-market.webp', '/photos/nishiki-stall.webp'],
  }),
  jp({
    id: 'p-yoshimura',
    name: 'Arashiyama Yoshimura (soba)',
    kind: 'food',
    lat: 35.0133,
    lng: 135.6786,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '3 Sagatenryuji Susukinobabacho, Ukyo',
    rating: 4.2,
    priceLevel: 2,
    hours: ['11:00–17:00 — river view seats go first'],
  }),
  jp({
    id: 'p-tokichi',
    name: 'Nakamura Tōkichi Honten (Uji)',
    kind: 'food',
    lat: 34.8894,
    lng: 135.8074,
    city: 'Uji',
    adminArea: 'Kyoto',
    address: '10 Ichiban, Uji',
    rating: 4.4,
    priceLevel: 2,
    hours: ['10:00–17:00'],
    photos: [
      '/photos/nakamura-tokichi.webp',
      '/photos/nakamura-tokichi-cafe.webp',
      '/photos/nakamura-tokichi-garden.webp',
    ],
  }),
  jp({
    id: 'p-todaiji',
    name: 'Tōdai-ji (Nara)',
    kind: 'sight',
    lat: 34.689,
    lng: 135.8398,
    city: 'Nara',
    adminArea: 'Nara',
    address: '406-1 Zoshicho, Nara',
    rating: 4.6,
    priceLevel: 1,
    hours: ['07:30–17:30'],
    photos: ['/photos/todaiji-daibutsuden.webp', '/photos/todaiji-daibutsuden-close.webp'],
  }),

  // --- Osaka -----------------------------------------------------------------
  jp({
    id: 'p-osakacastle',
    name: 'Osaka Castle',
    kind: 'sight',
    lat: 34.6873,
    lng: 135.5262,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: '1-1 Osakajo, Chuo',
    rating: 4.4,
    priceLevel: 1,
    hours: ['09:00–17:00, last entry 16:30'],
    photos: ['/photos/osaka-castle.webp'],
  }),
  jp({
    id: 'p-namba-hotel',
    name: 'Cross Hotel Osaka',
    kind: 'lodging',
    lat: 34.666,
    lng: 135.501,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: '2-5-15 Shinsaibashisuji, Chuo',
    rating: 4.3,
    priceLevel: 3,
  }),
  jp({
    id: 'p-dotonbori',
    name: 'Dōtonbori',
    kind: 'sight',
    lat: 34.6687,
    lng: 135.5013,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: 'Dotonbori, Chuo',
    rating: 4.4,
    hours: ['Best after dark'],
    photos: ['/photos/dotonbori.webp', '/photos/dotonbori-night.webp', '/photos/dotonbori-canal.webp'],
  }),
  jp({
    id: 'p-kuromon',
    name: 'Kuromon Ichiba Market',
    kind: 'food',
    lat: 34.6654,
    lng: 135.506,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: '2-4-1 Nipponbashi, Chuo',
    rating: 4.2,
    priceLevel: 2,
    hours: ['09:00–18:00'],
    photos: ['/photos/kuromon-market.webp'],
  }),
  jp({
    id: 'p-usj',
    name: 'Universal Studios Japan',
    kind: 'activity',
    lat: 34.6654,
    lng: 135.4323,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: '2-1-33 Sakurajima, Konohana',
    rating: 4.4,
    priceLevel: 4,
    website: 'https://www.usj.co.jp',
    hours: ['Varies; ~09:00–21:00'],
    photos: ['/photos/usj-hollywood.webp', '/photos/usj-gate.webp'],
  }),
  jp({
    id: 'p-kix',
    name: 'Kansai International Airport (KIX)',
    kind: 'transport_hub',
    lat: 34.4347,
    lng: 135.244,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: 'Izumisano, Osaka',
    photos: ['/photos/kansai-airport.webp', '/photos/kansai-airport-hall.webp'],
  }),
];

/**
 * Search-only place catalog — real, visitable spots NOT in the plan or on the
 * shortlist. `searchPlaces` matches over these plus the trip's own places, so
 * the add-stop composer's "Search places…" box can surface somewhere genuinely
 * new. They carry no photos (they're candidates for adoption, not yet cached).
 * Coordinates are real-world; adopting one mints a fresh Place from its draft.
 */
export const catalog: Place[] = [
  // --- Tokyo ---------------------------------------------------------------
  jp({
    id: 'cat-shibuyasky',
    name: 'Shibuya Sky',
    kind: 'sight',
    lat: 35.6581,
    lng: 139.7017,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '2-24-12 Shibuya, Shibuya City',
    rating: 4.5,
    priceLevel: 2,
    website: 'https://www.shibuya-scramble-square.com/sky/',
    hours: ['10:00–22:30, last entry 21:20'],
  }),
  jp({
    id: 'cat-skytree',
    name: 'Tokyo Skytree',
    kind: 'sight',
    lat: 35.7101,
    lng: 139.8107,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-1-2 Oshiage, Sumida City',
    rating: 4.5,
    priceLevel: 3,
    website: 'https://www.tokyo-skytree.jp',
    hours: ['09:00–22:00'],
  }),
  jp({
    id: 'cat-gyoen',
    name: 'Shinjuku Gyoen National Garden',
    kind: 'sight',
    lat: 35.6852,
    lng: 139.71,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '11 Naitomachi, Shinjuku City',
    rating: 4.6,
    priceLevel: 1,
    hours: ['09:00–16:30, closed Mon'],
  }),
  jp({
    id: 'cat-uenopark',
    name: 'Ueno Park',
    kind: 'sight',
    lat: 35.7148,
    lng: 139.7731,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: 'Uenokoen, Taito City',
    rating: 4.4,
    hours: ['05:00–23:00'],
  }),
  jp({
    id: 'cat-toyosu',
    name: 'Toyosu Market',
    kind: 'food',
    lat: 35.6459,
    lng: 139.7855,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '6-6-1 Toyosu, Koto City',
    rating: 4.1,
    priceLevel: 2,
    hours: ['05:00–15:00, closed Sun'],
  }),
  jp({
    id: 'cat-goldengai',
    name: 'Shinjuku Golden Gai',
    kind: 'food',
    lat: 35.694,
    lng: 139.7047,
    city: 'Tokyo',
    adminArea: 'Tokyo',
    address: '1-1-6 Kabukicho, Shinjuku City',
    rating: 4.3,
    priceLevel: 2,
    hours: ['Most bars 19:00–02:00'],
  }),

  // --- Hakone ----------------------------------------------------------------
  jp({
    id: 'cat-openair',
    name: 'Hakone Open-Air Museum',
    kind: 'activity',
    lat: 35.2447,
    lng: 139.0497,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: '1121 Ninotaira, Hakone',
    rating: 4.5,
    priceLevel: 2,
    website: 'https://www.hakone-oam.or.jp',
    hours: ['09:00–17:00'],
  }),
  jp({
    id: 'cat-ashi',
    name: 'Lake Ashi (Ashinoko)',
    kind: 'sight',
    lat: 35.2039,
    lng: 139.0161,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: 'Moto-Hakone, Hakone',
    rating: 4.4,
    hours: ['Sightseeing cruises ~09:30–16:30'],
  }),
  jp({
    id: 'cat-gorapark',
    name: 'Hakone Gōra Park',
    kind: 'sight',
    lat: 35.2436,
    lng: 139.0475,
    city: 'Hakone',
    adminArea: 'Kanagawa',
    address: '1300 Gora, Hakone',
    rating: 4.2,
    priceLevel: 1,
    hours: ['09:00–17:00'],
  }),

  // --- Kyoto -----------------------------------------------------------------
  jp({
    id: 'cat-kinkakuji',
    name: 'Kinkaku-ji (Golden Pavilion)',
    kind: 'sight',
    lat: 35.0394,
    lng: 135.7292,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '1 Kinkakujicho, Kita Ward',
    rating: 4.6,
    priceLevel: 1,
    hours: ['09:00–17:00'],
  }),
  jp({
    id: 'cat-ginkakuji',
    name: 'Ginkaku-ji (Silver Pavilion)',
    kind: 'sight',
    lat: 35.027,
    lng: 135.7983,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '2 Ginkakujicho, Sakyo Ward',
    rating: 4.5,
    priceLevel: 1,
    hours: ['08:30–17:00'],
  }),
  jp({
    id: 'cat-philosopher',
    name: "Philosopher's Path",
    kind: 'sight',
    lat: 35.0264,
    lng: 135.7944,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: 'Sakyo Ward, Kyoto',
    rating: 4.4,
    hours: ['Always open'],
  }),
  jp({
    id: 'cat-tenryuji',
    name: 'Tenryū-ji',
    kind: 'sight',
    lat: 35.0158,
    lng: 135.6739,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '68 Sagatenryuji Susukinobabacho, Ukyo Ward',
    rating: 4.5,
    priceLevel: 1,
    hours: ['08:30–17:00'],
  }),
  jp({
    id: 'cat-kokedera',
    name: 'Saihō-ji (Kokedera / Moss Temple)',
    kind: 'sight',
    lat: 34.9917,
    lng: 135.6836,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: '56 Matsuo Jingatanicho, Nishikyo Ward',
    rating: 4.5,
    priceLevel: 2,
    hours: ['Reservation required by postcard/online'],
  }),
  jp({
    id: 'cat-pontocho',
    name: 'Pontochō Alley',
    kind: 'food',
    lat: 35.0053,
    lng: 135.7706,
    city: 'Kyoto',
    adminArea: 'Kyoto',
    address: 'Pontocho, Nakagyo Ward',
    rating: 4.4,
    priceLevel: 3,
    hours: ['Restaurants from ~17:00'],
  }),

  // --- Uji (Kyoto day-trip range) -------------------------------------------
  jp({
    id: 'cat-byodoin',
    name: 'Byōdō-in',
    kind: 'sight',
    lat: 34.8892,
    lng: 135.8077,
    city: 'Uji',
    adminArea: 'Kyoto',
    address: '116 Renge, Uji',
    rating: 4.5,
    priceLevel: 1,
    hours: ['08:30–17:30'],
  }),

  // --- Nara (Kyoto day-trip range) ------------------------------------------
  jp({
    id: 'cat-narapark',
    name: 'Nara Park',
    kind: 'sight',
    lat: 34.6851,
    lng: 135.843,
    city: 'Nara',
    adminArea: 'Nara',
    address: 'Nara Park, Nara',
    rating: 4.6,
    hours: ['Always open — deer roam freely'],
  }),
  jp({
    id: 'cat-kasuga',
    name: 'Kasuga Taisha',
    kind: 'sight',
    lat: 34.6817,
    lng: 135.8481,
    city: 'Nara',
    adminArea: 'Nara',
    address: '160 Kasugano-cho, Nara',
    rating: 4.5,
    priceLevel: 1,
    hours: ['06:30–17:30'],
  }),

  // --- Osaka -----------------------------------------------------------------
  jp({
    id: 'cat-umedasky',
    name: 'Umeda Sky Building',
    kind: 'sight',
    lat: 34.7052,
    lng: 135.4901,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: '1-1-88 Oyodonaka, Kita Ward',
    rating: 4.4,
    priceLevel: 2,
    website: 'https://www.skybldg.co.jp',
    hours: ['09:30–22:30'],
  }),
  jp({
    id: 'cat-shinsekai',
    name: 'Shinsekai & Tsūtenkaku',
    kind: 'sight',
    lat: 34.6524,
    lng: 135.5061,
    city: 'Osaka',
    adminArea: 'Osaka',
    address: 'Ebisuhigashi, Naniwa Ward',
    rating: 4.3,
    priceLevel: 2,
    hours: ['Tower 10:00–20:00'],
  }),
];

export const placeById = new Map(places.map((p) => [p.id, p]));
