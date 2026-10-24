import { confirm } from "@tauri-apps/plugin-dialog";
import { api, type AiPack, type AiStatus, type IndexCount } from "../api";
import { formatBytes } from "../format";

type Props = {
  status: AiStatus | null;
  /** The install in progress, with bytes done. */
  installing: { pack: AiPack; done: number; total: number } | null;
  onChanged: () => void;
  onNotice: (message: string) => void;
  onError: (e: unknown) => void;
};

type About = {
  name: string;
  what: string;
  detail: string;
  /** What removing it deletes besides its model. */
  made: string | null;
  /** What indexing does, for the progress line ("Faces found in"). */
  indexing: string | null;
};

const ABOUT: Record<AiPack, About> = {
  search: {
    name: "Search by description",
    what: "Type what's in a photo (“beach at sunset”, “birthday cake”, “red car”) and find it, even without tags. Also adds ready-made searches like Food, Pets and Documents to the sidebar.",
    detail:
      "Uses OpenAI's CLIP model (MIT license). After installing, your photos are indexed in the background from their thumbnails; it pauses while folders are scanned and while the computer runs on battery.",
    made: "the search index",
    indexing: "Indexed",
  },
  people: {
    name: "People",
    what: "Finds the faces in your photos and groups each person's photos together. Name someone once to find all their photos; fix mistakes with “Not this person”, and merge a person found twice.",
    detail:
      "Uses OpenCV's YuNet (finding faces, MIT license) and SFace (telling faces apart, Apache 2.0). Faces are personal data: they stay in your library on this computer, are never uploaded, and are deleted completely when you remove this feature. Photos are looked at in the background, pausing for scans and on battery.",
    made: "every face found and every person, with their names",
    indexing: "Looked for faces in",
  },
  upscale: {
    name: "Upscale",
    what: "Enlarges small or old photos 2× or 4× with believable detail instead of blur. Choose Enlarge when exporting.",
    detail:
      "Uses Real-ESRGAN (BSD license). Photos up to 2048 pixels across are enlarged; each takes from a few seconds to a minute or two, so it's done only when you export.",
    made: null,
    indexing: null,
  },
  text: {
    name: "Text in photos",
    what: "Finds screenshots, receipts, documents and signs by the words in them (“wifi password”, “invoice 4521”) from the same search box, and shows a photo's text in its info so you can copy it.",
    detail:
      "Uses the text recognition built into Windows, so there's nothing to download. Photos are read in the background, pausing for scans and on battery.",
    made: "the text read from your photos",
    indexing: "Read text in",
  },
};

function progressLine(about: About, p: IndexCount | null, indexing: boolean) {
  if (!p || !about.indexing) return null;
  if (p.indexed >= p.total) return `All ${p.total.toLocaleString()} photos done`;
  return `${about.indexing} ${p.indexed.toLocaleString()} of ${p.total.toLocaleString()} photos${
    indexing ? "…" : " (continues when scans finish and on mains power)"
  }`;
}

/** Settings for the optional AI features: what each does, its size, install and remove. */
export function AiPage({ status, installing, onChanged, onNotice, onError }: Props) {
  if (!status) return null;

  const install = (pack: AiPack) => api.aiInstall(pack).then(onChanged).catch(onError);

  const remove = async (pack: AiPack, downloaded: boolean) => {
    const about = ABOUT[pack];
    const deleted = downloaded
      ? about.made
        ? `Its model files and ${about.made} are deleted.`
        : "Its model files are deleted."
      : `${about.made ?? "What it made"} is deleted.`.replace(/^t/, "T");
    const ok = await confirm(
      `${downloaded ? "Remove" : "Turn off"} ${about.name}? ${deleted} Your photos aren't touched, and you can ${downloaded ? "install" : "turn"} it again later${downloaded ? "" : " on"}.`,
      { title: "Remove AI feature", kind: "warning", okLabel: downloaded ? "Remove" : "Turn off" },
    );
    if (!ok) return;
    try {
      const now = await api.aiRemove(pack);
      onNotice(now ? `${about.name} removed.` : `${about.name} removed; some files are deleted when the app restarts.`);
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
          Optional, and off until you install them. Each runs on this computer only: your photos never leave it. Models
          are downloaded once (from Hugging Face, Microsoft and Qualcomm), checked before use, and can be removed at any
          time.
        </p>
      </section>
      {status.features.map((f) => {
        const about = ABOUT[f.id];
        const mine = installing?.pack === f.id ? installing : null;
        const busy = status.installing !== null;
        const progress = f.id === "upscale" ? null : status.progress[f.id];
        const line = progressLine(about, progress, status.indexing);
        return (
          <section key={f.id} className="ai-feature">
            <div className="ai-feature-head">
              <h3>{about.name}</h3>
              {f.installed && <span className="chip">{f.downloadBytes === 0 ? "On" : "Installed"}</span>}
            </div>
            <p>{about.what}</p>
            <p className="muted small">{about.detail}</p>
            {f.id === "text" && status.textLanguages.length > 0 && (
              <p className="muted small">Reads: {status.textLanguages.join(", ")}.</p>
            )}
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
                {line && <span className="muted small">{line}</span>}
                <div className="spacer" />
                <button onClick={() => remove(f.id, f.downloadBytes > 0)}>
                  {f.downloadBytes === 0 ? "Turn off" : "Remove"}
                </button>
              </div>
            ) : f.unavailable ? (
              <div className="ai-actions">
                <span className="warn small">{f.unavailable}</span>
              </div>
            ) : (
              <div className="ai-actions">
                <span className="muted small">
                  {f.downloadBytes === 0
                    ? "Nothing to download"
                    : `Download ${formatBytes(f.downloadBytes)} · ${formatBytes(f.diskBytes)} on disk`}
                </span>
                <div className="spacer" />
                <button className="primary" onClick={() => install(f.id)} disabled={busy}>
                  {f.downloadBytes === 0 ? "Turn on" : "Install"}
                </button>
              </div>
            )}
          </section>
        );
      })}
    </div>
  );
}
