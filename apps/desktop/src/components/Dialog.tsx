// Modal dialog primitive used by every Phase-2 admin dialog.
//
// We roll this by hand rather than pulling in a full dialog library
// because the dialogs are few, their behaviour is constrained, and a
// dependency on Radix / Headless UI would dwarf what we actually
// need. The primitive covers the three table-stakes modal affordances
// WCAG 2.1 expects: backdrop click closes, `Escape` closes, and focus
// is restored to the opener on unmount.
//
// Focus *trapping* within the dialog is intentionally not implemented.
// Every admin dialog is small (≤ 5 focusable controls), the app
// window already has no "outside" to tab into (Tauri webview), and a
// real focus trap is the kind of thing you want to share-a-dep or
// ship a dedicated component for, not hand-roll. If a later screen
// grows past the point where the native tab order is confusing,
// extract to `@dayseam/ui` and add a trap then.

import {
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
} from "react";

export interface DialogProps {
  /** Controls whether the dialog is mounted. */
  open: boolean;
  /** Fired when the user requests dismissal (Escape / backdrop click
   *  / the default "Close" button). The parent decides whether to
   *  honour it, which matters for dialogs with unsaved state. */
  onClose: () => void;
  /** The dialog's accessible name; also rendered as the header title
   *  so the visible title and `aria-labelledby` stay in sync. */
  title: string;
  /** Optional supporting copy under the title. */
  description?: string;
  /** Dialog body — typically a `<form>` or a vertical stack. */
  children: ReactNode;
  /** Actions row at the bottom of the dialog (buttons). Usually the
   *  primary + cancel pair, but any buttons are fine. */
  footer?: ReactNode;
  /** Width preset. `"md"` is the default and fits every dialog we
   *  ship today; `"lg"` is for ApproveReposDialog's list. */
  size?: "md" | "lg";
  /** Data-testid for the outer dialog element so tests can target it
   *  without depending on the visible title. */
  testId?: string;
}

const SIZE_CLASSES = {
  md: "w-[480px]",
  lg: "w-[640px]",
} as const;

export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  size = "md",
  testId,
}: DialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  // `previousFocusRef` snapshots which element owned focus at the
  // moment the dialog opened so we can restore it on close. Without
  // this the user lands back at `<body>` which is disorienting after
  // opening the dialog from a specific button.
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    // Move focus onto the dialog frame itself so the screen reader
    // announces the title immediately; individual dialogs can focus a
    // specific input afterwards via an autoFocus prop on their first
    // field.
    dialogRef.current?.focus();
    return () => {
      previousFocusRef.current?.focus?.();
    };
  }, [open]);

  const handleKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    },
    [onClose],
  );

  const handleBackdrop = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      // Only close when the click started *on* the backdrop, not on a
      // child that bubbled up. Otherwise drag-selecting text inside an
      // input and releasing outside it would close the dialog.
      if (event.target === event.currentTarget) onClose();
    },
    [onClose],
  );

  if (!open) return null;

  const titleId = `${testId ?? "dialog"}-title`;
  const descriptionId = description
    ? `${testId ?? "dialog"}-description`
    : undefined;

  return (
    <div
      role="presentation"
      onMouseDown={handleBackdrop}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
      data-testid={testId ? `${testId}-backdrop` : undefined}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        tabIndex={-1}
        onKeyDown={handleKeyDown}
        data-testid={testId}
        className={`${SIZE_CLASSES[size]} max-h-[min(720px,90vh)] overflow-hidden rounded-lg border border-neutral-200 bg-white shadow-xl outline-none dark:border-neutral-800 dark:bg-neutral-950`}
      >
        <header className="flex flex-col gap-1 border-b border-neutral-200 px-5 py-4 dark:border-neutral-800">
          <h2
            id={titleId}
            className="text-sm font-semibold text-neutral-900 dark:text-neutral-50"
          >
            {title}
          </h2>
          {description ? (
            <p
              id={descriptionId}
              className="text-xs text-neutral-600 dark:text-neutral-400"
            >
              {description}
            </p>
          ) : null}
        </header>
        <div className="max-h-[min(560px,70vh)] overflow-y-auto px-5 py-4 text-sm text-neutral-800 dark:text-neutral-200">
          {children}
        </div>
        {footer ? (
          <footer className="flex items-center justify-end gap-2 border-t border-neutral-200 bg-neutral-50 px-5 py-3 dark:border-neutral-800 dark:bg-neutral-900/60">
            {footer}
          </footer>
        ) : null}
      </div>
    </div>
  );
}

/** Utility wrapper so every dialog gets the same primary-button look
 *  without each component having to remember the Tailwind class
 *  soup. Kept here because `Dialog` is the only importer. */
export interface DialogButtonProps {
  kind: "primary" | "danger" | "secondary";
  type?: "button" | "submit";
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
}

// DAY-127: every kind carries an explicit 1px border (transparent for
// primary/danger, coloured for secondary) so all three variants have
// the same outer box height. Before this, the secondary button sat
// 2px taller than the primary because only secondary declared
// `border`, which meant footers with both Cancel + primary (or a
// standalone primary next to a secondary in the same dialog body)
// showed a tiny vertical misalignment — most visible on
// `IdentityManagerDialog` where a secondary "Add" inside the form
// sits directly above the primary "Done" in the footer.
const KIND_CLASSES: Record<DialogButtonProps["kind"], string> = {
  primary:
    "border border-transparent bg-neutral-900 text-white hover:bg-neutral-800 disabled:bg-neutral-400 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-white dark:disabled:bg-neutral-700",
  danger:
    "border border-transparent bg-red-600 text-white hover:bg-red-700 disabled:bg-red-300 dark:disabled:bg-red-900",
  secondary:
    "border border-neutral-300 bg-white text-neutral-800 hover:bg-neutral-50 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:hover:bg-neutral-800",
};

export function DialogButton({
  kind,
  type = "button",
  disabled,
  onClick,
  children,
}: DialogButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled}
      onClick={onClick}
      className={`rounded px-3 py-1.5 text-sm font-medium leading-5 transition disabled:cursor-not-allowed ${KIND_CLASSES[kind]}`}
    >
      {children}
    </button>
  );
}
