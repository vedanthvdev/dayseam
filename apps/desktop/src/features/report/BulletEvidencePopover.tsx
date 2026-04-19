// "Where did this bullet come from?" popover. Opens against a bullet
// in `StreamingPreview`, loads the full `ActivityEvent` rows for the
// ids the draft's `Evidence` records, and surfaces each event's
// `Link`s as clickable chips that route through the scoped
// `shell_open` IPC.
//
// The popover is rendered inline (absolutely positioned next to the
// bullet) rather than portalled because the preview scrolls as one
// piece and the popover should scroll with it. Escape + backdrop
// click close it via the shared behaviour on the parent bullet's
// click handler (owner state in `StreamingPreview`).

import { useEffect, useState } from "react";
import type { ActivityEvent, RenderedBullet } from "@dayseam/ipc-types";
import { invoke } from "../../ipc";

export interface BulletEvidencePopoverProps {
  bullet: RenderedBullet;
  eventIds: string[];
  reason: string;
  onClose: () => void;
}

function formatWhen(iso: string): string {
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    });
  } catch {
    return iso;
  }
}

export function BulletEvidencePopover({
  bullet,
  eventIds,
  reason,
  onClose,
}: BulletEvidencePopoverProps) {
  const [events, setEvents] = useState<ActivityEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setEvents(null);
    setError(null);
    void invoke("activity_events_get", { ids: eventIds })
      .then((rows) => {
        if (cancelled) return;
        setEvents(rows);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : JSON.stringify(err));
      });
    return () => {
      cancelled = true;
    };
  }, [eventIds]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  const openLink = async (url: string) => {
    try {
      await invoke("shell_open", { url });
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    }
  };

  return (
    <div
      role="dialog"
      aria-label={`Evidence for: ${bullet.text}`}
      data-testid={`evidence-popover-${bullet.id}`}
      className="absolute left-6 top-6 z-20 w-[420px] max-w-[90vw] rounded-md border border-neutral-200 bg-white p-3 shadow-lg dark:border-neutral-800 dark:bg-neutral-950"
    >
      <header className="flex items-start justify-between gap-2 pb-2">
        <p className="text-xs text-neutral-600 dark:text-neutral-400">
          {reason}
        </p>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close evidence"
          className="rounded border border-neutral-300 px-1.5 text-xs text-neutral-700 hover:bg-neutral-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-900"
        >
          ×
        </button>
      </header>

      {error ? (
        <p role="alert" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      ) : null}

      {events === null && !error ? (
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          Loading evidence…
        </p>
      ) : null}

      {events && events.length === 0 ? (
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          The events that produced this bullet are no longer on disk.
          Retention may have evicted them.
        </p>
      ) : null}

      {events && events.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {events.map((ev) => (
            <li
              key={ev.id}
              className="flex flex-col gap-1 rounded border border-neutral-200 px-2 py-1.5 dark:border-neutral-800"
            >
              <p className="text-xs font-medium text-neutral-800 dark:text-neutral-200">
                {ev.title}
              </p>
              <p className="text-[11px] text-neutral-500 dark:text-neutral-400">
                {formatWhen(ev.occurred_at)} · {ev.actor.display_name}
              </p>
              {ev.links.length > 0 ? (
                <div className="flex flex-wrap items-center gap-1 pt-0.5">
                  {ev.links.map((link) => (
                    <button
                      key={link.url}
                      type="button"
                      onClick={() => void openLink(link.url)}
                      data-testid={`evidence-link-${ev.id}`}
                      className="rounded bg-sky-100 px-1.5 py-0.5 text-[11px] text-sky-800 hover:bg-sky-200 dark:bg-sky-950 dark:text-sky-200 dark:hover:bg-sky-900"
                      title={link.url}
                    >
                      {link.label ?? link.url}
                    </button>
                  ))}
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
