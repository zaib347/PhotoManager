import { confirm } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { api, events, thumbUrl, type SimilarGroup, type SimilarOverview } from "../api";
import { formatBytes, formatTakenAt } from "../format";
import { describeReport, RecentlyRemoved } from "./RecentlyRemoved";

const PAGE = 30;

const WHY = {
  favorite: "Your favorite",
  rated: "Rated highest",
  sharpest: "Sharpest",
  exposure: "Best exposed",
  biggest: "Biggest",
  similar: "Best",
} as const;

const NOTES = { blurry: "Blurry", dark: "Too dark", bright: "Overexposed" } as const;

type Props = {
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

/** The photos ticked to keep in a group: the user's choice, else the suggested best. */
const keptIn = (g: SimilarGroup, choice: Record<number, boolean>) =>
  new Set(g.members.filter((m) => choice[m.assetId] ?? m.best).map((m) => m.assetId));

/** Groups of near-identical photos; pick the ones to keep, remove the rest (undoable). */
export function SimilarPage({ onNotice, onError }: Props) {
  const [overview, setOverview] = useState<SimilarOverview | null>(null);
  const [groups, setGroups] = useState<SimilarGroup[]>([]);
  /** Keep ticks the user changed, by asset id. */
  const [choice, setChoice] = useState<Record<number, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [removals, setRemovals] = useState(0);

  const reload = useCallback(async (count = PAGE) => {
    const [o, g] = await Promise.all([api.similarOverview(), api.similarGroups(0, count)]);
    setOverview(o);
    setGroups(g);
  }, []);

  useEffect(() => {
    reload().catch(onError);
    // Hashing runs in the background; groups fill in as it finishes.
    const off = events.similarChanged(() => reload().catch(onError));
    return () => {
      off.then((f) => f());
    };
  }, [reload, onError]);

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

  const removeUnkept = (list: SimilarGroup[]) => {
    const ids = list.flatMap((g) => {
      const kept = keptIn(g, choice);
      return kept.size === 0 ? [] : g.members.filter((m) => !kept.has(m.assetId)).map((m) => m.assetId);
    });
    return act(async () => describeReport(await api.removeSimilar(ids), "Moved"));
  };

  const removeAllShown = async () => {
    const count = groups.reduce((n, g) => {
      const kept = keptIn(g, choice);
      return n + (kept.size === 0 ? 0 : g.members.length - kept.size);
    }, 0);
    const ok = await confirm(
      `Remove ${count.toLocaleString()} photos from the ${groups.length.toLocaleString()} groups shown, keeping the ticked ones?\n\n` +
        "They move to a hidden .photomanager-trash folder next to them, and you can put them back from Recently removed.",
      { title: "Remove similar photos", kind: "warning", okLabel: "Remove photos" },
    );
    if (ok) await removeUnkept(groups);
  };

  const keepAll = (g: SimilarGroup) =>
    act(async () => {
      await api.keepSimilar(g.members.map((m) => m.assetId));
      return "Kept them all. This group won't be shown again.";
    });

  const more = () =>
    api
      .similarGroups(groups.length, PAGE)
      .then((next) => setGroups((g) => [...g, ...next]))
      .catch(onError);

  if (!overview) return null;

  return (
    <div className="events-page cleanup-page">
      <section className="cleanup-summary">
        <h2>Similar photos</h2>
        {overview.pending > 0 && (
          <p className="muted small">
            Still comparing {overview.pending.toLocaleString()} photos in the background. More groups may appear.
          </p>
        )}
        {overview.groups === 0 ? (
          <p className="muted">
            {overview.pending > 0 ? "No similar photos found yet." : "No similar photos found."}
          </p>
        ) : (
          <>
            <p>
              {overview.groups.toLocaleString()} groups of photos that look nearly the same (
              {overview.photos.toLocaleString()} photos): bursts, retakes and re-saved copies.
            </p>
            <button className="primary" disabled={busy} onClick={removeAllShown}>
              Remove unticked photos in all groups shown
            </button>
            <p className="muted small">
              The best photo of each group is ticked: your favorites and ratings first, then the shot with the
              sharpest focus and best exposure (judged on the full photo), or the biggest copy. Change the ticks to
              keep others.
            </p>
          </>
        )}
      </section>

      {groups.map((g) => {
        const kept = keptIn(g, choice);
        const removing = g.members.length - kept.size;
        return (
          <section key={g.id} className="similar-group">
            <div className="dupe-head">
              <span>{formatTakenAt(g.members[0].takenAt, g.members[0].tzOffsetMin)}</span>
              <span className="muted small">
                {g.kind === "copies" ? "Same picture, saved more than once" : `${g.members.length} similar shots`}
              </span>
              <div className="spacer" />
              <button disabled={busy} onClick={() => keepAll(g)}>
                Keep all
              </button>
              <button disabled={busy || kept.size === 0 || removing === 0} onClick={() => removeUnkept([g])}>
                {removing > 0 ? `Remove ${removing}` : "Remove"}
              </button>
            </div>
            <div className="similar-strip">
              {g.members.map((m) => {
                const keep = kept.has(m.assetId);
                return (
                  <label key={m.assetId} className={`similar-tile${keep ? " kept" : ""}`}>
                    <img src={thumbUrl(m.assetId)} alt="" loading="lazy" />
                    <span className="similar-meta small">
                      <input
                        type="checkbox"
                        checked={keep}
                        onChange={() => setChoice((c) => ({ ...c, [m.assetId]: !keep }))}
                      />
                      {keep ? "Keep" : "Remove"}
                      {m.best && (
                        <span className="badge" title="Suggested to keep">
                          {WHY[m.why ?? "similar"]}
                        </span>
                      )}
                    </span>
                    {m.notes.length > 0 && (
                      <span className="warn small">{m.notes.map((n) => NOTES[n]).join(" · ")}</span>
                    )}
                    <span className="muted small">
                      {m.width && m.height ? `${m.width}×${m.height} · ` : ""}
                      {formatBytes(m.bytes)}
                      {m.favorite ? " · ♥" : ""}
                      {m.rating ? ` · ${"★".repeat(m.rating)}` : ""}
                    </span>
                  </label>
                );
              })}
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
