import type { User } from '../../types';

/** The signed-in user in all mock sessions. */
export const ME = 'u-kaiyu';

/**
 * Mock cast: Kaiyu + the Phantom Thieves (Persona 5, © Atlus) as sample
 * users. Names only — no game assets, art, or dialogue are used anywhere.
 */
export const users: User[] = [
  { id: 'u-kaiyu', email: 'kaiyu.huang@proton.me', displayName: 'Kaiyu', avatarColor: '#6b5bd2' },
  { id: 'u-makoto', email: 'makoto.niijima@example.com', displayName: 'Makoto', avatarColor: '#a0522d' },
  { id: 'u-ryuji', email: 'ryuji.sakamoto@example.com', displayName: 'Ryuji', avatarColor: '#e6b422' },
  { id: 'u-ann', email: 'ann.takamaki@example.com', displayName: 'Ann', avatarColor: '#e05263' },
  { id: 'u-yusuke', email: 'yusuke.kitagawa@example.com', displayName: 'Yusuke', avatarColor: '#3b6fd4' },
  { id: 'u-futaba', email: 'futaba.sakura@example.com', displayName: 'Futaba', avatarColor: '#4fb06d' },
];
