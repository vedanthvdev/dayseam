// Dialog: manage the `SourceIdentity` rows attached to the canonical
// self-`Person`. An identity row maps a per-source actor (a git
// author email, a GitLab user id / username, a GitHub login) back to
// the Dayseam user so the reporting engine can answer "was *I* the
// author?" without string-matching the display name.
//
// v0.1's rule: identities are edited exactly-as-stored. No fuzzy
// matching, no "oh we think this email is you" suggestions. That
// lives in v0.2 when cross-source ambiguity actually shows up.
//
// The dialog resolves the self-person lazily: if `persons_get_self`
// fails the dialog degrades to a read-only error banner rather than
// blocking the rest of the app.

import { useCallback, useEffect, useState } from "react";
import type {
  Person,
  Source,
  SourceIdentity,
  SourceIdentityKind,
  SourceKind,
} from "@dayseam/ipc-types";
import { useIdentities, useSources } from "../../ipc";
import { invoke } from "../../ipc/invoke";
import { Dialog, DialogButton } from "../../components/Dialog";
import { ConnectorLogo } from "../../components/ConnectorLogo";

interface IdentityManagerDialogProps {
  open: boolean;
  onClose: () => void;
}

const IDENTITY_KINDS: readonly { value: SourceIdentityKind; label: string }[] = [
  { value: "GitEmail", label: "Git email" },
  { value: "GitLabUserId", label: "GitLab user id" },
  { value: "GitLabUsername", label: "GitLab username" },
  { value: "GitHubLogin", label: "GitHub login" },
];

/** DAY-170: map an identity row back to the connector kind it
 *  belongs to so we can render the right coloured brand mark next
 *  to it — the same affordance chip rows already have, extended
 *  inward to the identity manager so the user doesn't have to
 *  cross-reference kind strings to figure out which service an
 *  email or login maps to.
 *
 *  If the identity carries a specific `source_id` we prefer the
 *  actual source's kind (the user scoped it, so we know for sure).
 *  Otherwise we fall back to the identity kind's natural owner —
 *  `GitEmail` is the only ambiguous case (it could belong to any
 *  git-based source) and defaults to `LocalGit`, which is also the
 *  kind git-email identities were originally introduced for.
 *
 *  Returns `null` for an identity kind that has no visual connector
 *  mapping yet (future-proofing; today every `SourceIdentityKind`
 *  resolves). `null` means "render nothing" rather than "render a
 *  generic fallback" — we'd rather be silent than lie. */
function identityConnector(
  identity: SourceIdentity,
  sources: Source[],
): SourceKind | null {
  if (identity.source_id !== null) {
    const scoped = sources.find((s) => s.id === identity.source_id);
    if (scoped) return scoped.kind;
  }
  switch (identity.kind) {
    case "GitEmail":
      return "LocalGit";
    case "GitLabUserId":
    case "GitLabUsername":
      return "GitLab";
    case "GitHubLogin":
      return "GitHub";
    default:
      return null;
  }
}

function generateIdentityId(): string {
  // `crypto.randomUUID` exists in every Tauri-supported webview; the
  // `?? fallback` keeps tests that run before jsdom ever saw `crypto`
  // from crashing on import.
  if (typeof globalThis.crypto?.randomUUID === "function") {
    return globalThis.crypto.randomUUID();
  }
  return `id-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function IdentityManagerDialog({
  open,
  onClose,
}: IdentityManagerDialogProps) {
  const [self, setSelf] = useState<Person | null>(null);
  const [selfError, setSelfError] = useState<string | null>(null);
  const { sources } = useSources();
  const {
    identities,
    loading,
    error,
    upsert,
    remove,
  } = useIdentities(self?.id ?? null);

  const [kind, setKind] = useState<SourceIdentityKind>("GitEmail");
  const [value, setValue] = useState("");
  const [sourceId, setSourceId] = useState<string>("");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setSelfError(null);
    void invoke("persons_get_self", {})
      .then((person) => {
        if (!cancelled) setSelf(person);
      })
      .catch((err) => {
        if (!cancelled)
          setSelfError(err instanceof Error ? err.message : JSON.stringify(err));
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const handleAdd = useCallback(async () => {
    if (!self || value.trim().length === 0) return;
    setSubmitting(true);
    setFormError(null);
    try {
      const identity: SourceIdentity = {
        id: generateIdentityId(),
        person_id: self.id,
        source_id: sourceId === "" ? null : sourceId,
        kind,
        external_actor_id: value.trim(),
      };
      await upsert(identity);
      setValue("");
    } catch (err) {
      setFormError(err instanceof Error ? err.message : JSON.stringify(err));
    } finally {
      setSubmitting(false);
    }
  }, [self, value, sourceId, kind, upsert]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Identity mappings"
      description={
        self
          ? `Tell Dayseam which external accounts belong to you. Used to filter reports to your own work across ${sources.length} source${sources.length === 1 ? "" : "s"}.`
          : "Loading self-person…"
      }
      size="lg"
      testId="identity-manager-dialog"
      footer={
        <DialogButton kind="primary" onClick={onClose}>
          Done
        </DialogButton>
      }
    >
      {selfError ? (
        <p
          role="alert"
          className="mb-3 rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
        >
          Could not load the self-person: {selfError}
        </p>
      ) : null}

      {self ? (
        <>
          <form
            className="mb-4 flex flex-col gap-2 rounded border border-neutral-200 bg-neutral-50 p-3 dark:border-neutral-800 dark:bg-neutral-900/50"
            onSubmit={(e) => {
              e.preventDefault();
              void handleAdd();
            }}
          >
            <span className="text-xs font-medium text-neutral-700 dark:text-neutral-300">
              Add mapping for {self.display_name}
            </span>
            <div className="flex flex-wrap items-center gap-2">
              <select
                value={kind}
                onChange={(e) => setKind(e.target.value as SourceIdentityKind)}
                className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
                aria-label="Identity kind"
              >
                {IDENTITY_KINDS.map((k) => (
                  <option key={k.value} value={k.value}>
                    {k.label}
                  </option>
                ))}
              </select>
              <input
                type="text"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder={kind === "GitEmail" ? "me@example.com" : "value"}
                className="min-w-[200px] flex-1 rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
                aria-label="Identity value"
              />
              <select
                value={sourceId}
                onChange={(e) => setSourceId(e.target.value)}
                className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
                aria-label="Scope to source"
              >
                <option value="">Any source</option>
                {sources.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                  </option>
                ))}
              </select>
              <DialogButton
                kind="secondary"
                type="submit"
                disabled={submitting || value.trim().length === 0}
              >
                {submitting ? "Adding…" : "Add"}
              </DialogButton>
            </div>
            {formError ? (
              <p
                role="alert"
                className="text-xs text-red-600 dark:text-red-400"
              >
                {formError}
              </p>
            ) : null}
          </form>

          {loading && identities.length === 0 ? (
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              Loading mappings…
            </p>
          ) : null}

          {error ? (
            <p
              role="alert"
              className="rounded border border-red-300 bg-red-50 px-3 py-2 text-xs text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
            >
              Failed to load identities: {error}
            </p>
          ) : null}

          {identities.length === 0 && !loading && !error ? (
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              No mappings yet. Add your git email at minimum so commit
              authorship lines up.
            </p>
          ) : null}

          {identities.length > 0 ? (
            <ul className="flex flex-col divide-y divide-neutral-200 dark:divide-neutral-800">
              {identities.map((id) => (
                <IdentityRow
                  key={id.id}
                  identity={id}
                  sources={sources}
                  onDelete={() => void remove(id.id)}
                />
              ))}
            </ul>
          ) : null}
        </>
      ) : null}
    </Dialog>
  );
}

function IdentityRow({
  identity,
  sources,
  onDelete,
}: {
  identity: SourceIdentity;
  sources: Source[];
  onDelete: () => void;
}) {
  const kindLabel =
    IDENTITY_KINDS.find((k) => k.value === identity.kind)?.label ?? identity.kind;
  const sourceLabel =
    identity.source_id === null
      ? "Any source"
      : (sources.find((s) => s.id === identity.source_id)?.label ??
        "Unknown source");
  // DAY-170: surface the coloured brand mark so the user sees at a
  // glance that e.g. `me@acme.test` belongs to LocalGit / GitHub /
  // GitLab — the same visual convention the sources strip uses.
  const connector = identityConnector(identity, sources);
  return (
    <li
      className="flex items-center justify-between gap-3 py-2"
      data-testid={`identity-row-${identity.id}`}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        {connector ? (
          // `ConnectorLogo` already emits its own
          // `data-testid="connector-logo-<Kind>"`; scoping tests
          // inside the row's `data-testid` wrapper keeps the assertion
          // "this identity row carries a GitHub mark" easy to write
          // without another layer of bespoke testids.
          <ConnectorLogo
            kind={connector}
            size={16}
            colored
            className="shrink-0"
          />
        ) : null}
        <div className="flex min-w-0 flex-col">
          <span className="text-xs font-medium text-neutral-600 dark:text-neutral-400">
            {kindLabel} · {sourceLabel}
          </span>
          <span className="truncate font-mono text-sm text-neutral-900 dark:text-neutral-100">
            {identity.external_actor_id}
          </span>
        </div>
      </div>
      <button
        type="button"
        onClick={onDelete}
        aria-label={`Delete mapping ${identity.external_actor_id}`}
        className="rounded border border-neutral-300 px-2 py-1 text-xs text-neutral-700 hover:bg-red-50 hover:text-red-700 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-red-950/40 dark:hover:text-red-300"
      >
        Remove
      </button>
    </li>
  );
}
