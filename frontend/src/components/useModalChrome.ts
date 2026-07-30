import { useEffect, useRef } from 'react';

/**
 * The four things every modal in this app was missing.
 *
 * All of them are invisible until you stop using a mouse, which is exactly why
 * they went unnoticed: opening any composer left focus on the trigger *behind*
 * the backdrop, so the first Tab walked the trip cards underneath it, Escape
 * worked but Tab never came back, and the page scrolled under the sheet while
 * a phone user tried to scroll the sheet itself.
 *
 *   1. Focus moves into the dialog on open (the first control, or the dialog).
 *   2. Tab and Shift-Tab cycle inside it and cannot leave.
 *   3. The page behind is scroll-locked, with the scrollbar's width paid back
 *      as padding so the layout does not jump on desktop.
 *   4. Focus returns to whatever opened it.
 *
 * Returns the ref to attach to the dialog element.
 *
 * Escape is deliberately NOT handled here: both callers already own it, and
 * both have to, because Escape is stacked — a photo lightbox above the modal
 * takes it first.
 */
export function useModalChrome<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);

  useEffect(() => {
    const node = ref.current;
    const opener = document.activeElement as HTMLElement | null;

    const selector =
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
    const focusable = () =>
      Array.from(node?.querySelectorAll<HTMLElement>(selector) ?? []).filter(
        (el) => el.offsetWidth > 0 || el.offsetHeight > 0 || el === document.activeElement,
      );

    // Prefer a text field, then any control, then the dialog itself. Landing on
    // the close button is technically inside the trap but reads as hostile.
    const first = focusable();
    const entry =
      first.find((el) => el.matches('input, textarea')) ?? first.find((el) => !el.matches('.x, .compose-x'));
    if (entry) entry.focus();
    else node?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || !node) return;
      const items = focusable();
      if (!items.length) return;
      const edge = e.shiftKey ? items[0] : items[items.length - 1];
      const wrap = e.shiftKey ? items[items.length - 1] : items[0];
      if (document.activeElement === edge || !node.contains(document.activeElement)) {
        e.preventDefault();
        wrap.focus();
      }
    };
    document.addEventListener('keydown', onKey, true);

    // Nested surfaces (a lightbox over a composer) must not each restore their
    // own padding on the way out, so the compensation is only applied by the
    // first one to lock.
    const body = document.body;
    const wasLocked = body.style.overflow === 'hidden';
    const gap = window.innerWidth - document.documentElement.clientWidth;
    if (!wasLocked) {
      body.style.overflow = 'hidden';
      if (gap > 0) body.style.paddingRight = `${gap}px`;
    }

    return () => {
      document.removeEventListener('keydown', onKey, true);
      if (!wasLocked) {
        body.style.overflow = '';
        body.style.paddingRight = '';
      }
      opener?.focus?.();
    };
  }, []);

  return ref;
}
