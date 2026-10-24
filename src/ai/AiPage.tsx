import { confirm } from "@tauri-apps/plugin-dialog";
import { api, type AiPack, type AiStatus } from "../api";
import { formatBytes } from "../format";

type Props = {
  status: AiStatus | null;
  /** The install in progress, with bytes done. */
  installing: { pack: AiPack; done: number; total: number } | null;
  onChanged: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

const ABOUT: Record<AiPack, { name: string; what: string; detail: string }> = {
  search: {
    name: "Search by description",
    what: "Type what's in a photo (“beach at sunset”, “birthday cake”, “red car”) and find it, even without tags. Also adds ready-made searches like Food, Pets and Documents to the sidebar.",
    detail:
      "Uses OpenAI's CLIP model (MIT license). After installing, your photos are indexed in the background from their thumbnails; it pauses while folders are scanned and while the computer runs on battery.",
  },
};

/** Settings for the optional AI features: what each does, its size, install and remove. */
export function AiPage({ status, installing, onChanged, onNotice, onError }: Props) {
  if (!status) return null;

  const install = (pack: AiPack) =>
    api
      .aiInstall(pack)
      .then(onChanged)
      .catch(onError);

  const remove = async (pack: AiPack) => {
    const ok = await confirm(
      `Remove ${ABOUT[pack].name}? Its model files and everything it made (the search index) are deleted. Your photos aren't touched, and you can install it again later.`,
      { title: "Remove AI feature", kind: "warning", okLabel: "Remove" },
    );
    if (!ok) return;
    try {
      const now = await api.aiRemove(pack);
      onNotice(now ? `${ABOUT[pack].name} removed.` : `${ABOUT[pack].name} removed; some files are deleted when the app restarts.`);
      onChanged();
    } catch (e) {
      onError(e);
    }
  };

  return (
    <div className="events-page ai-page">
      <section className="cleanup-summary">
        <h2>AI features</h2>
        <p className="muted small">
          Optional, and off until you install them. Each runs on this computer only: your photos never leave it. The
          models are downloaded once (from Hugging Face and Microsoft), checked before use, and can be removed at any
          time.
        </p>
      </section>
      {status.features.map((f) => {
        const about = ABOUT[f.id];
        const mine = installing?.pack === f.id ? installing : null;
        const busy = status.installing !== null;
        return (
          <section key={f.id} className="ai-feature">
            <div className="ai-feature-head">
              <h3>{about.name}</h3>
              {f.installed && <span className="chip">Installed</span>}
            </div>
            <p>{about.what}</p>
            <p className="muted small">{about.detail}</p>
            {mine ? (
              <div className="ai-progress">
                <div className="progress">
                  <div className="progress-bar" style={{ width: `${(mine.done / Math.max(1, mine.total)) * 100}%` }} />
                </div>
                <span className="muted small">
                  Downloading {formatBytes(mine.done)} of {formatBytes(mine.total)}
                </span>
                <button onClick={() => api.aiCancelInstall().catch(onError)}>Stop</button>
              </div>
            ) : f.installed ? (
              <div className="ai-actions">
                {f.id === "search" && status.search && (
                  <span className="muted small">
                    {status.search.indexed >= status.search.total
                      ? `All ${status.search.total.toLocaleString()} photos indexed`
                      : `${status.indexing ? "Indexing" : "Indexed"} ${status.search.indexed.toLocaleString()} of ${status.search.total.toLocaleString()} photos${status.indexing ? "…" : " (continues when scans finish and on mains power)"}`}
                  </span>
                )}
                <div className="spacer" />
                <button onClick={() => remove(f.id)}>Remove</button>
              </div>
            ) : (
              <div className="ai-actions">
                <span className="muted small">
                  Download {formatBytes(f.downloadBytes)} · {formatBytes(f.diskBytes)} on disk
                </span>
                <div className="spacer" />
                <button className="primary" onClick={() => install(f.id)} disabled={busy}>
                  Install
                </button>
              </div>
            )}
          </section>
        );
      })}
      <section className="ai-feature muted">
        <h3>Coming next</h3>
        <p className="small">People (grouping faces, opt-in on its own) and Upscale (enlarging small photos).</p>
      </section>
    </div>
  );
}

