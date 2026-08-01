import { commonCatalog } from './common';
import { languageCatalog } from './language';
import { navigationCatalog } from './navigation';
import { shellCatalog } from './shell';
import { themeCatalog } from './theme';
import { tripCatalog } from './trip';

/**
 * Feature catalogs stay in separate files so page owners can translate their
 * own namespace. Add each new pair here with one English and one Chinese
 * spread; TypeScript then checks that the assembled languages have equal keys.
 */
export const coreEnglish = {
  ...commonCatalog.en,
  ...languageCatalog.en,
  ...shellCatalog.en,
  ...themeCatalog.en,
  ...navigationCatalog.en,
  ...tripCatalog.en,
} as const;

export const coreSimplifiedChinese: { [Key in keyof typeof coreEnglish]: string } = {
  ...commonCatalog['zh-CN'],
  ...languageCatalog['zh-CN'],
  ...shellCatalog['zh-CN'],
  ...themeCatalog['zh-CN'],
  ...navigationCatalog['zh-CN'],
  ...tripCatalog['zh-CN'],
};
