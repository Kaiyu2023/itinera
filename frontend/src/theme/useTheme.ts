import { useSyncExternalStore } from 'react';
import type { Theme } from '../lib/oklch';

/**
 * Light / dark, chosen rather than inherited.
 *
 * The app had no toggle: dark mode existed only as `@media
 * (prefers-color-scheme: dark)`, so on a laptop pinned to light there was no
 * way to see it, and no way to read a plan in dark on a phone that is not.
 *
 * `system` is the default and is a real third state, not "whichever it resolved
 * to at first paint" — a trip read on a phone with automatic switching should
 * follow the phone across sunset, which is the exact hour this app is about.
 *
 * The resolved value lands on `<html data-theme>`, and every dark rule in the
 * stylesheets keys off that attribute rather than off the media query. That is
 * what makes one switch able to override the OS. `index.html` sets the same
 * attribute in a blocking inline script before first paint, so there is no
 * flash of the wrong theme — this module and that script must agree about the
 * storage key and the values, which is why both are stated here.
 */

const KEY = 'itinera.theme';
const MEDIA = '(prefers-color-scheme: dark)';

export type ThemeChoice = 'system' | 'light' | 'dark';

export const THEME_CHOICES: ThemeChoice[] = ['light', 'system', 'dark'];

function readChoice(): ThemeChoice {
  try {
    const raw = localStorage.getItem(KEY);
    return raw === 'light' || raw === 'dark' ? raw : 'system';
  } catch {
    return 'system';
  }
}

function systemTheme(): Theme {
  return window.matchMedia(MEDIA).matches ? 'dark' : 'light';
}

export function resolve(choice: ThemeChoice): Theme {
  return choice === 'system' ? systemTheme() : choice;
}

/* ---- store ---------------------------------------------------------------
   A three-line store rather than a context, because `useColorScheme` is called
   from leaves all over the tree (every surface that re-synthesises an accent)
   and threading a provider through them all to carry one enum is not worth it.
   `useSyncExternalStore` gets the tearing behaviour right for free. */

let choice: ThemeChoice = typeof localStorage === 'undefined' ? 'system' : readChoice();
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  // While the choice is `system`, an OS flip has to reach every subscriber.
  const mq = window.matchMedia(MEDIA);
  const onMedia = () => {
    if (choice === 'system') {
      apply();
      fn();
    }
  };
  mq.addEventListener('change', onMedia);
  return () => {
    listeners.delete(fn);
    mq.removeEventListener('change', onMedia);
  };
}

/** Paint the resolved theme onto the document. `color-scheme` goes with it so
    native widgets — date pickers, select popups, scrollbars, autofill — follow
    the choice rather than the OS. */
function apply() {
  const theme = resolve(choice);
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.style.colorScheme = theme;
}

export function setThemeChoice(next: ThemeChoice) {
  choice = next;
  try {
    if (next === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, next);
  } catch {
    /* private mode — the choice still holds for this session */
  }
  apply();
  emit();
}

export function useThemeChoice(): ThemeChoice {
  return useSyncExternalStore(
    subscribe,
    () => choice,
    () => 'system' as ThemeChoice,
  );
}

/**
 * The colour scheme actually in force. Everything that re-derives a colour in
 * JS — the OKLCH accent synthesis, above all — must read this and not the media
 * query directly, or a manually darkened page gets an accent built for a light
 * substrate.
 */
export function useResolvedTheme(): Theme {
  return useSyncExternalStore(
    subscribe,
    () => resolve(choice),
    () => 'light' as Theme,
  );
}
