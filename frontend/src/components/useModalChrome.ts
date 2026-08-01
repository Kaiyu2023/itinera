import { useEffect, useRef } from 'react';

/**
 * Modal surfaces can legitimately stack (for example, the add-stop composer
 * opened from the full-screen mobile map). Only the top surface may own focus
 * or be exposed as modal; otherwise two document-level focus traps tug focus
 * back and forth and assistive technology sees two simultaneous modals.
 */
const modalStack: HTMLElement[] = [];

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

    const covered = modalStack.at(-1);
    const coveredAriaHidden = covered?.getAttribute('aria-hidden') ?? null;
    const coveredWasInert = covered?.inert ?? false;
    if (covered) {
      covered.setAttribute('aria-hidden', 'true');
      covered.inert = true;
    }
    if (node) modalStack.push(node);

    const selector =
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
    const focusable = () =>
      Array.from(node?.querySelectorAll<HTMLElement>(selector) ?? []).filter(
        (el) => el.offsetWidth > 0 || el.offsetHeight > 0 || el === document.activeElement,
      );

    // The first control in DOM order that is not a dismiss button, then the
    // dialog itself. Landing on the close button is technically inside the trap
    // but reads as hostile.
    //
    // It deliberately does NOT prefer a text field. That was the first rule
    // here, and it broke the add-stop sheet the moment that surface got a
    // proper scrolling body: the first text field is the "Why" box near the
    // bottom of a 705px form in a 489px window, so focusing it scrolled the
    // sheet to its end and the composer opened with the map and the mode toggle
    // above the fold. `preventScroll` alone is the wrong fix — it would leave
    // the focus ring somewhere off-screen, which is worse than what it
    // replaces. Entering at the top and letting the user Tab down is both
    // sound and boring.
    const entry = focusable().find((el) => !el.matches('.x, .compose-x, .close, .lb-close'));
    (entry ?? node)?.focus({ preventScroll: true });

    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || !node || modalStack.at(-1) !== node) return;
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
      if (node) {
        const index = modalStack.lastIndexOf(node);
        if (index >= 0) modalStack.splice(index, 1);
      }
      if (covered && modalStack.at(-1) === covered) {
        if (coveredAriaHidden == null) covered.removeAttribute('aria-hidden');
        else covered.setAttribute('aria-hidden', coveredAriaHidden);
        covered.inert = coveredWasInert;
      }
      if (!wasLocked) {
        body.style.overflow = '';
        body.style.paddingRight = '';
      }
      opener?.focus?.();
    };
  }, []);

  return ref;
}
