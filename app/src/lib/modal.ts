/** Keyboard behaviour every dialog needs: focus moves in, Tab stays inside, Escape closes, and
 *  focus goes back to where it was when the dialog closes. Use as `use:modal={{ onClose }}`. */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function modal(node: HTMLElement, opts: { onClose?: () => void } = {}) {
  let options = opts;
  const before = document.activeElement as HTMLElement | null;
  node.setAttribute("aria-modal", "true");
  if (!node.hasAttribute("tabindex")) node.tabIndex = -1;

  const items = () => [...node.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
  // Wait a moment so the dialog's own contents exist before choosing the first control.
  const timer = setTimeout(() => (items()[0] ?? node).focus(), 0);

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      options.onClose?.();
      return;
    }
    if (e.key !== "Tab") return;
    const list = items();
    if (list.length === 0) {
      e.preventDefault();
      return;
    }
    const first = list[0];
    const last = list[list.length - 1];
    const at = document.activeElement;
    if (!node.contains(at)) {
      e.preventDefault();
      first.focus();
      return;
    }
    if (e.shiftKey && (at === first || at === node)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && at === last) {
      e.preventDefault();
      first.focus();
    }
  }
  node.addEventListener("keydown", key);

  return {
    update(next: { onClose?: () => void }) {
      options = next;
    },
    destroy() {
      clearTimeout(timer);
      node.removeEventListener("keydown", key);
      before?.focus?.();
    },
  };
}
