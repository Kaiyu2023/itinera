export type LocaleCatalog<T extends Record<string, string>> = {
  en: T;
  'zh-CN': { [Key in keyof T]: string };
};

/** Keep every namespace's English and Simplified-Chinese keys in lockstep. */
export function defineCatalog<const T extends Record<string, string>>(
  en: T,
  simplifiedChinese: { [Key in keyof T]: string },
): LocaleCatalog<T> {
  return { en, 'zh-CN': simplifiedChinese };
}
