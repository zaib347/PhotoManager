import { confirm } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbUrl,
  type DuplicateGroup,
  type DuplicateOverview,
  type FileInfo,
  type KeepPreference,
} from "../api";
import { fileName, formatBytes, formatTakenAt } from "../format";
import { describeReport, RecentlyRemoved } from "./RecentlyRemoved";

const PAGE = 50;

const PREFER: { id: KeepPreference; label: string; hint: string }[] = [
  {
    id: "suggested",
    label: "Suggested",
    hint: "the one that looks like the original",
  },
  {
    id: "oldest",
    label: "Oldest",
    hint: "the one with the oldest modified date",
  },
  {
    id: "newest",
    label: "Newest",
    hint: "the one with the newest modified date",
  },
];

const modified = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "short",
  year: "numeric",
});

/** The copy `prefer` keeps; mirrors `duplicates::preferred` (copies come suggested-first). */
function preferred(
  copies: FileInfo[],
  prefer: KeepPreference,
): number | undefined {
  const present = copies.filter((f) => !f.missing);
  const online = present.filter((f) => f.online);
  const pool = online.length > 0 ? online : present;
  if (prefer === "suggested" || pool.length === 0)
    return pool[0]?.id ?? copies[0]?.id;
  return pool.reduce((a, b) =>
    prefer === "oldest"
      ? b.modifiedAt < a.modifiedAt
        ? b
        : a
      : b.modifiedAt > a.modifiedAt
        ? b
        : a,
  ).id;
}

type Props = {
  /** Bumped by the parent after scans, which can find new copies. */
  version: number;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

/** Photos stored more than once, and the removals that can still be undone. */
export function DuplicatesPage({ version, onNotice, onError }: Props) {
  const [overview, setOverview] = useState<DuplicateOverview | null>(null);
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  /** Which copy to keep in every group, unless picked by hand below. */
  const [prefer, setPrefer] = useState<KeepPreference>("suggested");
  /** Copies picked by hand, by asset. */
  const [keep, setKeep] = useState<Record<number, number>>({});
  const [busy, setBusy] = useState(false);
  // Bumped after removals so Recently removed reloads.
  const [removals, setRemovals] = useState(0);

  const reload = useCallback(async (count = PAGE) => {
    const [o, g] = await Promise.all([
      api.duplicateOverview(),
      api.duplicateGroups(0, count),
    ]);
    setOverview(o);
    setGroups(g);
  }, []);

  useEffect(() => {
    reload().catch(onError);
  }, [version, reload, onError]);

  const act = async (action: () => Promise<string>) => {
    setBusy(true);
    try {
      onNotice(await action());
      setRemovals((n) => n + 1);
      await reload(Math.max(PAGE, groups.length));
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };

  const choose = (next: KeepPreference) => {
    setPrefer(next);
    // A new rule for every group replaces the picks made by hand.
    setKeep({});
  };

  const removeAll = async () => {
    if (!overview) return;
    const rule = PREFER.find((p) => p.id === prefer)!;
    const picked = Object.keys(keep).length;
    const ok = await confirm(
      `Remove ${overview.extraFiles.toLocaleString()} extra copies (${formatBytes(overview.extraBytes)})?\n\n` +
        `Each photo keeps one copy: ${rule.hint}${picked > 0 ? `, or the one you picked (${picked.toLocaleString()} photos)` : ""}. ` +
        "The others move to a hidden .photomanager-trash folder next to them, and you can put them back from " +
        "Recently removed.",
      {
        title: "Remove extra copies",
        kind: "warning",
        okLabel: "Yes",
        cancelLabel: "No",
      },
    );
    if (ok)
      await act(async () =>
        describeReport(await api.removeDuplicates(keep, prefer), "Moved"),
      );
  };

  const removeOne = async (g: DuplicateGroup, kept: FileInfo | undefined) => {
    const others = g.copies.length - 1;
    const ok = await confirm(
      `Keep "${kept ? fileName(kept.path) : "this copy"}" and remove the other ${others === 1 ? "copy" : `${others} copies`}?\n\n` +
        "You can put them back from Recently removed.",
      {
        title: "Remove other copies",
        kind: "warning",
        okLabel: "Yes",
        cancelLabel: "No",
      },
    );
    if (ok)
      await act(async () =>
        describeReport(
          await api.removeDuplicates(keep, prefer, [g.assetId]),
          "Moved",
        ),
      );
  };

  const more = () =>
    api
      .duplicateGroups(groups.length, PAGE)
      .then((next) => setGroups((g) => [...g, ...next]))
      .catch(onError);

  if (!overview) return null;

  return (
    <div className="events-page cleanup-page">
      <section className="cleanup-summary">
        <h2>Duplicates</h2>
        {overview.groups === 0 ? (
          <p className="muted">
            No photos or videos are stored more than once.
          </p>
        ) : (
          <>
            <p>
              {overview.groups.toLocaleString()}{" "}
              {overview.groups === 1 ? "item has" : "items have"}{" "}
              {overview.extraFiles.toLocaleString()} extra{" "}
              {overview.extraFiles === 1 ? "copy" : "copies"}, using{" "}
              <strong>{formatBytes(overview.extraBytes)}</strong>.
            </p>
            <div className="dupe-actions">
              <span className="muted small">Keep</span>
              <div
                className="segmented"
                role="radiogroup"
                aria-label="Which copy to keep"
              >
                {PREFER.map((p) => (
                  <button
                    key={p.id}
                    role="radio"
                    aria-checked={prefer === p.id}
                    className={prefer === p.id ? "on" : ""}
                    disabled={busy}
                    onClick={() => choose(p.id)}
                  >
                    {p.label}
                  </button>
                ))}
              </div>
              <button className="primary" disabled={busy} onClick={removeAll}>
                Remove extra copies
              </button>
            </div>
            <p className="muted small">
              Keeps one copy of each photo:{" "}
              {PREFER.find((p) => p.id === prefer)!.hint}, unless you pick
              another below. Removed copies can be put back.
            </p>
          </>
        )}
      </section>

      {groups.map((g) => {
        const kept = keep[g.assetId] ?? preferred(g.copies, prefer);
        return (
          <section key={g.assetId} className="dupe-group">
            <img
              className="dupe-thumb"
              src={thumbUrl(g.assetId)}
              alt=""
              loading="lazy"
            />
            <div className="dupe-body">
              <div className="dupe-head">
                <span>{formatTakenAt(g.takenAt, g.tzOffsetMin)}</span>
                <span className="muted small">
                  {formatBytes(g.size)} × {g.copies.length}
                </span>
                <div className="spacer" />
                <button
                  disabled={busy}
                  onClick={() =>
                    removeOne(
                      g,
                      g.copies.find((f) => f.id === kept),
                    )
                  }
                >
                  Remove other copies
                </button>
              </div>
              <ul className="dupe-copies">
                {g.copies.map((f) => (
                  <li key={f.id} className={kept === f.id ? "kept" : ""}>
                    <label title={f.path}>
                      <input
                        type="radio"
                        name={`keep-${g.assetId}`}
                        checked={kept === f.id}
                        onChange={() =>
                          setKeep((k) => ({ ...k, [g.assetId]: f.id }))
                        }
                      />
                      <span className="dupe-name">{fileName(f.path)}</span>
                      {kept === f.id && (
                        <span className="dupe-keep-chip">Keep</span>
                      )}
                      <span className="muted small file-path">{f.path}</span>
                      {!f.online && (
                        <span className="warn small">drive not connected</span>
                      )}
                      <span className="muted small dupe-date" title="Modified">
                        {modified.format(f.modifiedAt)}
                      </span>
                    </label>
                    {f.online && (
                      <button
                        className="link small"
                        onClick={() => revealItemInDir(f.path)}
                      >
                        Show
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          </section>
        );
      })}
      {groups.length < overview.groups && (
        <div className="cleanup-more">
          <button onClick={more}>Show more</button>
        </div>
      )}

      <RecentlyRemoved
        version={removals}
        onChanged={() => reload(Math.max(PAGE, groups.length)).catch(onError)}
        onNotice={onNotice}
        onError={onError}
      />
    </div>
  );
}
