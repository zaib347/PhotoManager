import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AssetDetails } from "../api";
import { fileName, formatBytes, formatDuration, formatOffset, formatTakenAt } from "../format";

const TIME_SOURCES = ["from photo metadata", "from video metadata", "from the file name", "from the file's modified date", "set by you"];

export function InfoPanel({ asset }: { asset: AssetDetails | null }) {
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
          <p className="muted">
            {asset.lat.toFixed(5)}, {asset.lon.toFixed(5)}
          </p>
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
