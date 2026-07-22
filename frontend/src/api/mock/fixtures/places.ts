import type { Place, PlaceKind } from '../../types';

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
  /** Repo-hosted WebPs (see public/ATTRIBUTIONS.md) — R2 keys in prod. */
  photos?: string[];
}

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
  };
}

export const places: Place[] = [
  // --- Tokyo ---------------------------------------------------------------
  jp({ id: 'p-haneda', name: 'Haneda Airport (HND)', kind: 'transport_hub', lat: 35.5494, lng: 139.7798, city: 'Tokyo', adminArea: 'Tokyo', address: 'Ota City, Tokyo', photos: ['/photos/haneda-airport.webp', '/photos/haneda-keikyu-sign.webp'] }),
  jp({ id: 'p-gracery', name: 'Hotel Gracery Shinjuku', kind: 'lodging', lat: 35.6952, lng: 139.7015, city: 'Tokyo', adminArea: 'Tokyo', address: '1-19-1 Kabukicho, Shinjuku', rating: 4.2, priceLevel: 3, website: 'https://shinjuku.gracery.com', photos: ['/photos/hotel-gracery-shinjuku.webp'] }),
  jp({ id: 'p-shibuya', name: 'Shibuya Scramble Crossing', kind: 'sight', lat: 35.6595, lng: 139.7005, city: 'Tokyo', adminArea: 'Tokyo', address: 'Shibuya City, Tokyo', rating: 4.4, photos: ['/photos/shibuya-crossing.webp', '/photos/shibuya-aerial.webp', '/photos/shibuya-2am.webp'] }),
  jp({ id: 'p-omoide', name: 'Omoide Yokocho', kind: 'food', lat: 35.6938, lng: 139.6993, city: 'Tokyo', adminArea: 'Tokyo', address: '1-2 Nishishinjuku, Shinjuku', rating: 4.3, priceLevel: 2, hours: ['Most stalls: 17:00–24:00'], photos: ['/photos/omoide-yokocho.webp', '/photos/omoide-yokocho-entrance.webp'] }),
  jp({ id: 'p-sensoji', name: 'Sensō-ji', kind: 'sight', lat: 35.7148, lng: 139.7967, city: 'Tokyo', adminArea: 'Tokyo', address: '2-3-1 Asakusa, Taito', rating: 4.5, hours: ['Grounds always open; main hall 06:00–17:00'], photos: ['/photos/sensoji-kaminarimon.webp', '/photos/sensoji-kaminarimon-night.webp', '/photos/sensoji-dragon-carving.webp'] }),
  jp({ id: 'p-teamlab', name: 'teamLab Planets', kind: 'activity', lat: 35.6494, lng: 139.7898, city: 'Tokyo', adminArea: 'Tokyo', address: '6-1-16 Toyosu, Koto', rating: 4.6, priceLevel: 3, website: 'https://www.teamlab.art/e/planets/', hours: ['09:00–22:00, timed entry'], photos: ['/photos/teamlab-planets.webp'] }),
  jp({ id: 'p-tsukiji', name: 'Tsukiji Outer Market', kind: 'food', lat: 35.6654, lng: 139.7707, city: 'Tokyo', adminArea: 'Tokyo', address: '4 Chome Tsukiji, Chuo', rating: 4.4, priceLevel: 2, hours: ['Shops ~05:00–14:00', 'Many shops closed Sun & Wed — go on a weekday morning'], photos: ['/photos/tsukiji-outer-market.webp', '/photos/tsukiji-stall.webp'] }),
  jp({ id: 'p-meiji', name: 'Meiji Jingū', kind: 'sight', lat: 35.6764, lng: 139.6993, city: 'Tokyo', adminArea: 'Tokyo', address: '1-1 Yoyogikamizonocho, Shibuya', rating: 4.6, hours: ['Sunrise to sunset'], photos: ['/photos/meiji-jingu-torii.webp', '/photos/meiji-torii-sun.webp'] }),
  jp({ id: 'p-ichiran', name: 'Ichiran Shibuya', kind: 'food', lat: 35.6613, lng: 139.7004, city: 'Tokyo', adminArea: 'Tokyo', address: '1-22-7 Jinnan, Shibuya', rating: 4.3, priceLevel: 2, hours: ['24 hours'] }),
  jp({ id: 'p-uobei', name: 'Uobei Sushi Shibuya', kind: 'food', lat: 35.6580, lng: 139.6994, city: 'Tokyo', adminArea: 'Tokyo', address: '2-29-11 Dogenzaka, Shibuya', rating: 4.1, priceLevel: 1, hours: ['11:00–23:00'] }),
  jp({ id: 'p-gyukatsu', name: 'Gyukatsu Motomura Shibuya', kind: 'food', lat: 35.6570, lng: 139.7016, city: 'Tokyo', adminArea: 'Tokyo', address: '3-18-10 Shibuya', rating: 4.5, priceLevel: 2, hours: ['10:00–22:00'] }),
  jp({ id: 'p-ghibli', name: 'Ghibli Museum', kind: 'activity', lat: 35.6962, lng: 139.5704, city: 'Mitaka', adminArea: 'Tokyo', address: '1-1-83 Shimorenjaku, Mitaka', rating: 4.5, priceLevel: 2, website: 'https://www.ghibli-museum.jp', hours: ['10:00–18:00, closed Tue', 'Tickets: lottery/advance only, on sale 10th of prior month 10:00 JST'], photos: ['/photos/ghibli-museum.webp'] }),
  jp({ id: 'p-gyozalou', name: 'Harajuku Gyōza Lou', kind: 'food', lat: 35.6690, lng: 139.7079, city: 'Tokyo', adminArea: 'Tokyo', address: '6-2-4 Jingumae, Shibuya', rating: 4.2, priceLevel: 1, hours: ['11:30–22:00 — expect a queue at noon'] }),
  jp({ id: 'p-samurai', name: 'Samurai Restaurant (Kabukichō)', kind: 'activity', lat: 35.6949, lng: 139.7020, city: 'Tokyo', adminArea: 'Tokyo', address: '1-7-7 Kabukicho, Shinjuku', rating: 3.9, priceLevel: 4, hours: ['Shows nightly, reservation required'] }),

  // --- Hakone ----------------------------------------------------------------
  jp({ id: 'p-odawara', name: 'Odawara Station', kind: 'transport_hub', lat: 35.2564, lng: 139.1553, city: 'Odawara', adminArea: 'Kanagawa', address: 'Odawara, Kanagawa' }),
  jp({ id: 'p-yumoto', name: 'Hakone-Yumoto Station', kind: 'transport_hub', lat: 35.2323, lng: 139.1069, city: 'Hakone', adminArea: 'Kanagawa', address: 'Yumoto, Hakone', hours: ['Coin lockers by the main exit'], photos: ['/photos/hakone-yumoto-station.webp', '/photos/hakone-yumoto-escalator.webp'] }),
  jp({ id: 'p-hakoneshrine', name: 'Hakone Shrine', kind: 'sight', lat: 35.2048, lng: 139.0250, city: 'Hakone', adminArea: 'Kanagawa', address: '80-1 Motohakone, Hakone', rating: 4.5, hours: ['Grounds always open'], photos: ['/photos/hakone-shrine.webp'] }),
  jp({ id: 'p-owakudani', name: 'Ōwakudani (Hakone Ropeway)', kind: 'sight', lat: 35.2444, lng: 139.0194, city: 'Hakone', adminArea: 'Kanagawa', address: 'Sengokuhara, Hakone', rating: 4.4, hours: ['Ropeway 09:00–17:00, weather-dependent'], photos: ['/photos/owakudani-fuji.webp'] }),
  jp({ id: 'p-ichinoyu', name: 'Ichinoyu Honkan (ryokan, est. 1630)', kind: 'lodging', lat: 35.2270, lng: 139.0968, city: 'Hakone', adminArea: 'Kanagawa', address: '90 Tonosawa, Hakone', rating: 4.2, priceLevel: 3, website: 'https://www.ichinoyu.co.jp', hours: ['Check-in 15:00, dinner seatings 17:30 / 19:30'] }),

  // --- Kyoto -----------------------------------------------------------------
  jp({ id: 'p-kyotostation', name: 'Kyoto Station', kind: 'transport_hub', lat: 34.9858, lng: 135.7588, city: 'Kyoto', adminArea: 'Kyoto', address: 'Higashishiokoji, Shimogyo', photos: ['/photos/kyoto-station.webp'] }),
  jp({ id: 'p-kyoto-hotel', name: 'Piece Hostel Sanjo', kind: 'lodging', lat: 35.0089, lng: 135.7622, city: 'Kyoto', adminArea: 'Kyoto', address: '531 Asakuracho, Nakagyo', rating: 4.4, priceLevel: 2 }),
  jp({ id: 'p-nijo', name: 'Nijō Castle', kind: 'sight', lat: 35.0142, lng: 135.7481, city: 'Kyoto', adminArea: 'Kyoto', address: '541 Nijojocho, Nakagyo', rating: 4.4, priceLevel: 1, hours: ['08:45–17:00, last entry 16:00'], photos: ['/photos/nijo-castle.webp'] }),
  jp({ id: 'p-gion', name: 'Gion (evening walk)', kind: 'sight', lat: 35.0037, lng: 135.7780, city: 'Kyoto', adminArea: 'Kyoto', address: 'Gion, Higashiyama', rating: 4.5, photos: ['/photos/gion-hanamikoji.webp'] }),
  jp({ id: 'p-fushimi', name: 'Fushimi Inari Taisha', kind: 'sight', lat: 34.9671, lng: 135.7727, city: 'Kyoto', adminArea: 'Kyoto', address: '68 Fukakusa Yabunouchicho, Fushimi', rating: 4.6, hours: ['Always open — go at dawn to beat crowds'], photos: ['/photos/fushimi-inari-torii.webp', '/photos/fushimi-inari-tunnel.webp'] }),
  jp({ id: 'p-kiyomizu', name: 'Kiyomizu-dera', kind: 'sight', lat: 34.9949, lng: 135.7850, city: 'Kyoto', adminArea: 'Kyoto', address: '1-294 Kiyomizu, Higashiyama', rating: 4.5, priceLevel: 1, hours: ['06:00–18:00, last entry 17:30'], photos: ['/photos/kiyomizu-november.webp', '/photos/kiyomizu-stage.webp'] }),
  jp({ id: 'p-arashiyama', name: 'Arashiyama Bamboo Grove', kind: 'sight', lat: 35.0170, lng: 135.6710, city: 'Kyoto', adminArea: 'Kyoto', address: 'Sagaogurayama, Ukyo', rating: 4.4, hours: ['Always open'], photos: ['/photos/arashiyama-bamboo.webp', '/photos/arashiyama-bamboo-canopy.webp'] }),
  jp({ id: 'p-nishiki', name: 'Nishiki Market', kind: 'food', lat: 35.0050, lng: 135.7649, city: 'Kyoto', adminArea: 'Kyoto', address: 'Nishikikoji-dori, Nakagyo', rating: 4.3, priceLevel: 2, hours: ['Most shops 10:00–17:00'], photos: ['/photos/nishiki-market.webp', '/photos/nishiki-stall.webp'] }),
  jp({ id: 'p-yoshimura', name: 'Arashiyama Yoshimura (soba)', kind: 'food', lat: 35.0133, lng: 135.6786, city: 'Kyoto', adminArea: 'Kyoto', address: '3 Sagatenryuji Susukinobabacho, Ukyo', rating: 4.2, priceLevel: 2, hours: ['11:00–17:00 — river view seats go first'] }),
  jp({ id: 'p-tokichi', name: 'Nakamura Tōkichi Honten (Uji)', kind: 'food', lat: 34.8894, lng: 135.8074, city: 'Uji', adminArea: 'Kyoto', address: '10 Ichiban, Uji', rating: 4.4, priceLevel: 2, hours: ['10:00–17:00'], photos: ['/photos/nakamura-tokichi.webp', '/photos/nakamura-tokichi-cafe.webp', '/photos/nakamura-tokichi-garden.webp'] }),
  jp({ id: 'p-todaiji', name: 'Tōdai-ji (Nara)', kind: 'sight', lat: 34.6890, lng: 135.8398, city: 'Nara', adminArea: 'Nara', address: '406-1 Zoshicho, Nara', rating: 4.6, priceLevel: 1, hours: ['07:30–17:30'], photos: ['/photos/todaiji-daibutsuden.webp', '/photos/todaiji-daibutsuden-close.webp'] }),

  // --- Osaka -----------------------------------------------------------------
  jp({ id: 'p-osakacastle', name: 'Osaka Castle', kind: 'sight', lat: 34.6873, lng: 135.5262, city: 'Osaka', adminArea: 'Osaka', address: '1-1 Osakajo, Chuo', rating: 4.4, priceLevel: 1, hours: ['09:00–17:00, last entry 16:30'], photos: ['/photos/osaka-castle.webp'] }),
  jp({ id: 'p-namba-hotel', name: 'Cross Hotel Osaka', kind: 'lodging', lat: 34.6660, lng: 135.5010, city: 'Osaka', adminArea: 'Osaka', address: '2-5-15 Shinsaibashisuji, Chuo', rating: 4.3, priceLevel: 3 }),
  jp({ id: 'p-dotonbori', name: 'Dōtonbori', kind: 'sight', lat: 34.6687, lng: 135.5013, city: 'Osaka', adminArea: 'Osaka', address: 'Dotonbori, Chuo', rating: 4.4, hours: ['Best after dark'], photos: ['/photos/dotonbori.webp', '/photos/dotonbori-night.webp', '/photos/dotonbori-canal.webp'] }),
  jp({ id: 'p-kuromon', name: 'Kuromon Ichiba Market', kind: 'food', lat: 34.6654, lng: 135.5060, city: 'Osaka', adminArea: 'Osaka', address: '2-4-1 Nipponbashi, Chuo', rating: 4.2, priceLevel: 2, hours: ['09:00–18:00'], photos: ['/photos/kuromon-market.webp'] }),
  jp({ id: 'p-usj', name: 'Universal Studios Japan', kind: 'activity', lat: 34.6654, lng: 135.4323, city: 'Osaka', adminArea: 'Osaka', address: '2-1-33 Sakurajima, Konohana', rating: 4.4, priceLevel: 4, website: 'https://www.usj.co.jp', hours: ['Varies; ~09:00–21:00'], photos: ['/photos/usj-hollywood.webp', '/photos/usj-gate.webp'] }),
  jp({ id: 'p-kix', name: 'Kansai International Airport (KIX)', kind: 'transport_hub', lat: 34.4347, lng: 135.2440, city: 'Osaka', adminArea: 'Osaka', address: 'Izumisano, Osaka', photos: ['/photos/kansai-airport.webp', '/photos/kansai-airport-hall.webp'] }),
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
  jp({ id: 'cat-shibuyasky', name: 'Shibuya Sky', kind: 'sight', lat: 35.6581, lng: 139.7017, city: 'Tokyo', adminArea: 'Tokyo', address: '2-24-12 Shibuya, Shibuya City', rating: 4.5, priceLevel: 2, website: 'https://www.shibuya-scramble-square.com/sky/', hours: ['10:00–22:30, last entry 21:20'] }),
  jp({ id: 'cat-skytree', name: 'Tokyo Skytree', kind: 'sight', lat: 35.7101, lng: 139.8107, city: 'Tokyo', adminArea: 'Tokyo', address: '1-1-2 Oshiage, Sumida City', rating: 4.5, priceLevel: 3, website: 'https://www.tokyo-skytree.jp', hours: ['09:00–22:00'] }),
  jp({ id: 'cat-gyoen', name: 'Shinjuku Gyoen National Garden', kind: 'sight', lat: 35.6852, lng: 139.7100, city: 'Tokyo', adminArea: 'Tokyo', address: '11 Naitomachi, Shinjuku City', rating: 4.6, priceLevel: 1, hours: ['09:00–16:30, closed Mon'] }),
  jp({ id: 'cat-uenopark', name: 'Ueno Park', kind: 'sight', lat: 35.7148, lng: 139.7731, city: 'Tokyo', adminArea: 'Tokyo', address: 'Uenokoen, Taito City', rating: 4.4, hours: ['05:00–23:00'] }),
  jp({ id: 'cat-toyosu', name: 'Toyosu Market', kind: 'food', lat: 35.6459, lng: 139.7855, city: 'Tokyo', adminArea: 'Tokyo', address: '6-6-1 Toyosu, Koto City', rating: 4.1, priceLevel: 2, hours: ['05:00–15:00, closed Sun'] }),
  jp({ id: 'cat-goldengai', name: 'Shinjuku Golden Gai', kind: 'food', lat: 35.6940, lng: 139.7047, city: 'Tokyo', adminArea: 'Tokyo', address: '1-1-6 Kabukicho, Shinjuku City', rating: 4.3, priceLevel: 2, hours: ['Most bars 19:00–02:00'] }),

  // --- Hakone ----------------------------------------------------------------
  jp({ id: 'cat-openair', name: 'Hakone Open-Air Museum', kind: 'activity', lat: 35.2447, lng: 139.0497, city: 'Hakone', adminArea: 'Kanagawa', address: '1121 Ninotaira, Hakone', rating: 4.5, priceLevel: 2, website: 'https://www.hakone-oam.or.jp', hours: ['09:00–17:00'] }),
  jp({ id: 'cat-ashi', name: 'Lake Ashi (Ashinoko)', kind: 'sight', lat: 35.2039, lng: 139.0161, city: 'Hakone', adminArea: 'Kanagawa', address: 'Moto-Hakone, Hakone', rating: 4.4, hours: ['Sightseeing cruises ~09:30–16:30'] }),
  jp({ id: 'cat-gorapark', name: 'Hakone Gōra Park', kind: 'sight', lat: 35.2436, lng: 139.0475, city: 'Hakone', adminArea: 'Kanagawa', address: '1300 Gora, Hakone', rating: 4.2, priceLevel: 1, hours: ['09:00–17:00'] }),

  // --- Kyoto -----------------------------------------------------------------
  jp({ id: 'cat-kinkakuji', name: 'Kinkaku-ji (Golden Pavilion)', kind: 'sight', lat: 35.0394, lng: 135.7292, city: 'Kyoto', adminArea: 'Kyoto', address: '1 Kinkakujicho, Kita Ward', rating: 4.6, priceLevel: 1, hours: ['09:00–17:00'] }),
  jp({ id: 'cat-ginkakuji', name: 'Ginkaku-ji (Silver Pavilion)', kind: 'sight', lat: 35.0270, lng: 135.7983, city: 'Kyoto', adminArea: 'Kyoto', address: '2 Ginkakujicho, Sakyo Ward', rating: 4.5, priceLevel: 1, hours: ['08:30–17:00'] }),
  jp({ id: 'cat-philosopher', name: "Philosopher's Path", kind: 'sight', lat: 35.0264, lng: 135.7944, city: 'Kyoto', adminArea: 'Kyoto', address: 'Sakyo Ward, Kyoto', rating: 4.4, hours: ['Always open'] }),
  jp({ id: 'cat-tenryuji', name: 'Tenryū-ji', kind: 'sight', lat: 35.0158, lng: 135.6739, city: 'Kyoto', adminArea: 'Kyoto', address: '68 Sagatenryuji Susukinobabacho, Ukyo Ward', rating: 4.5, priceLevel: 1, hours: ['08:30–17:00'] }),
  jp({ id: 'cat-kokedera', name: 'Saihō-ji (Kokedera / Moss Temple)', kind: 'sight', lat: 34.9917, lng: 135.6836, city: 'Kyoto', adminArea: 'Kyoto', address: '56 Matsuo Jingatanicho, Nishikyo Ward', rating: 4.5, priceLevel: 2, hours: ['Reservation required by postcard/online'] }),
  jp({ id: 'cat-pontocho', name: 'Pontochō Alley', kind: 'food', lat: 35.0053, lng: 135.7706, city: 'Kyoto', adminArea: 'Kyoto', address: 'Pontocho, Nakagyo Ward', rating: 4.4, priceLevel: 3, hours: ['Restaurants from ~17:00'] }),

  // --- Uji (Kyoto day-trip range) -------------------------------------------
  jp({ id: 'cat-byodoin', name: 'Byōdō-in', kind: 'sight', lat: 34.8892, lng: 135.8077, city: 'Uji', adminArea: 'Kyoto', address: '116 Renge, Uji', rating: 4.5, priceLevel: 1, hours: ['08:30–17:30'] }),

  // --- Nara (Kyoto day-trip range) ------------------------------------------
  jp({ id: 'cat-narapark', name: 'Nara Park', kind: 'sight', lat: 34.6851, lng: 135.8430, city: 'Nara', adminArea: 'Nara', address: 'Nara Park, Nara', rating: 4.6, hours: ['Always open — deer roam freely'] }),
  jp({ id: 'cat-kasuga', name: 'Kasuga Taisha', kind: 'sight', lat: 34.6817, lng: 135.8481, city: 'Nara', adminArea: 'Nara', address: '160 Kasugano-cho, Nara', rating: 4.5, priceLevel: 1, hours: ['06:30–17:30'] }),

  // --- Osaka -----------------------------------------------------------------
  jp({ id: 'cat-umedasky', name: 'Umeda Sky Building', kind: 'sight', lat: 34.7052, lng: 135.4901, city: 'Osaka', adminArea: 'Osaka', address: '1-1-88 Oyodonaka, Kita Ward', rating: 4.4, priceLevel: 2, website: 'https://www.skybldg.co.jp', hours: ['09:30–22:30'] }),
  jp({ id: 'cat-shinsekai', name: 'Shinsekai & Tsūtenkaku', kind: 'sight', lat: 34.6524, lng: 135.5061, city: 'Osaka', adminArea: 'Osaka', address: 'Ebisuhigashi, Naniwa Ward', rating: 4.3, priceLevel: 2, hours: ['Tower 10:00–20:00'] }),
];

export const placeById = new Map(places.map((p) => [p.id, p]));
