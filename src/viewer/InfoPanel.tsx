import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AssetDetails, View } from "../api";
import { useCollections } from "../collections/context";
import { TagInput } from "../collections/TagInput";
import { fileName, formatBytes, formatDuration, formatOffset, formatTakenAt, placeLabel } from "../format";
import { eventTitle } from "../events/format";

const TIME_SOURCES = ["from photo metadata", "from video metadata", "from the file name", "from the file's modified date", "set by you"];

type Props = {
  asset: AssetDetails | null;
  onAddTag: (name: string) => void;
  onRemoveTag: (tagId: number) => void;
  onNavigate: (view: View) => void;
  onRestoreDate: () => void;
};

export function InfoPanel({ asset, onAddTag, onRemoveTag, onNavigate, onRestoreDate }: Props) {
  const { tags } = useCollections();
  if (!asset) return <aside className="info-panel" />;
  const camera = [asset.cameraMake, asset.cameraModel].filter(Boolean).join(" ");
  // Many cameras repeat the make in the model ("Canon Canon EOS 80D").
  const cameraLabel =
    asset.cameraMake && asset.cameraModel?.toLowerCase().startsWith(asset.cameraMake.toLowerCase())
      ? asset.cameraModel
      : camera;
  const megapixels = asset.width && asset.height ? (asset.width * asset.height) / 1e6 : null;
  const uncertainTime = asset.takenSrc === 3;

  return (
    <aside className="info-panel">
      <section>
        <h3>Date</h3>
        <p>{formatTakenAt(asset.takenAt, asset.tzOffsetMin)}</p>
        <p className={uncertainTime ? "warn" : "muted"}>
          {TIME_SOURCES[asset.takenSrc] ?? "unknown source"}
          {asset.takenSrc === 0 && asset.tzOffsetMin !== 0 && ` · ${formatOffset(asset.tzOffsetMin)}`}
        </p>
        {uncertainTime && <p className="muted small">The real capture date may differ: this file had no date inside it.</p>}
        {asset.takenSrc === 4 && (
          <button className="link small" onClick={onRestoreDate}>
            Restore original date
          </button>
        )}
      </section>

      {(cameraLabel || asset.width) && (
        <section>
          <h3>Details</h3>
          {cameraLabel && <p>{cameraLabel}</p>}
          {asset.width && asset.height && (
            <p className="muted">
              {asset.width.toLocaleString()} × {asset.height.toLocaleString()}
              {megapixels && megapixels >= 0.5 && ` · ${megapixels.toFixed(1)} MP`}
            </p>
          )}
          {asset.durationMs !== null && <p className="muted">Duration {formatDuration(asset.durationMs)}</p>}
        </section>
      )}

      {asset.lat !== null && asset.lon !== null && (
        <section>
          <h3>Location</h3>
          {asset.place && <p>{placeLabel(asset.place)}</p>}
          <p className="muted">
            {asset.lat.toFixed(5)}, {asset.lon.toFixed(5)}
          </p>
        </section>
      )}

      <section>
        <h3>Tags</h3>
        <div className="chips">
          {asset.tags.map((t) => (
            <span key={t.id} className="chip">
              <button className="link" onClick={() => onNavigate({ tag: t.id })}>
                {t.name}
              </button>
              <button className="chip-remove" onClick={() => onRemoveTag(t.id)} aria-label={`Remove tag ${t.name}`}>
                ×
              </button>
            </span>
          ))}
        </div>
        <TagInput tags={tags} exclude={new Set(asset.tags.map((t) => t.id))} onAdd={onAddTag} />
      </section>

      {asset.event && (
        <section>
          <h3>Event</h3>
          <div className="chips">
            <button className="chip link" onClick={() => onNavigate({ event: asset.event!.id })}>
              {eventTitle(asset.event)}
            </button>
          </div>
        </section>
      )}

      {asset.albums.length > 0 && (
        <section>
          <h3>Albums</h3>
          <div className="chips">
            {asset.albums.map((a) => (
              <button key={a.id} className="chip link" onClick={() => onNavigate({ album: a.id })}>
                {a.name}
              </button>
            ))}
          </div>
        </section>
      )}

      <section>
        <h3>{asset.files.length === 1 ? "File" : `${asset.files.length} copies`}</h3>
        <ul className="file-list">
          {asset.files.map((f) => (
            <li key={f.id}>
              <div className="file-name" title={f.path}>
                {fileName(f.path)}
              </div>
              <div className="muted small file-path" title={f.path}>
                {f.path}
              </div>
              <div className="file-actions small">
                <span className="muted">
                  {formatBytes(f.size)}
                  {f.missing ? " · not found" : !f.online ? " · drive not connected" : ""}
                </span>
                {f.online && (
                  <button className="link" onClick={() => revealItemInDir(f.path)}>
                    Show in folder
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      </section>
    </aside>
  );
}
