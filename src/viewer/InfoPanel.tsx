import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { api, type AssetDetails, type View } from "../api";
import { useCollections } from "../collections/context";
import { TagInput } from "../collections/TagInput";
import { fileName, formatBytes, formatDuration, formatOffset, formatTakenAt, placeLabel } from "../format";
import { eventTitle } from "../events/format";
import { tripTitle } from "../trips/format";

const TIME_SOURCES = ["from photo metadata", "from video metadata", "from the file name", "from the file's modified date", "set by you", "from Google Photos or iCloud"];

type Props = {
  asset: AssetDetails | null;
  onAddTag: (name: string) => void;
  onRemoveTag: (tagId: number) => void;
  onNavigate: (view: View) => void;
  onRestoreDate: () => void;
  onSetDescription: (text: string) => void;
};

/** A description, saved when the field loses focus. */
function Description({ value, onSave }: { value: string; onSave: (text: string) => void }) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return (
    <textarea
      className="description"
      rows={2}
      placeholder="Add a description"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => text.trim() !== value && onSave(text)}
    />
  );
}

/** What the AI pack found in a photo: the named people in it and the text it reads. */
function AiInfo({ id, onNavigate }: { id: number; onNavigate: (view: View) => void }) {
  const [people, setPeople] = useState<[number, string][]>([]);
  const [text, setText] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let stale = false;
    setCopied(false);
    Promise.all([api.aiPhotoPeople(id), api.aiPhotoText(id)])
      .then(([p, t]) => {
        if (stale) return;
        setPeople(p);
        setText(t);
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [id]);

  return (
    <>
      {people.length > 0 && (
        <section>
          <h3>People</h3>
          <div className="chips">
            {people.map(([pid, name]) => (
              <button key={pid} className="chip link" onClick={() => onNavigate({ person: pid })}>
                {name}
              </button>
            ))}
          </div>
        </section>
      )}
      {text && (
        <section>
          <h3>Text in photo</h3>
          <p className="photo-text">{text}</p>
          <button
            className="link small"
            onClick={() =>
              navigator.clipboard
                .writeText(text)
                .then(() => setCopied(true))
                .catch(() => {})
            }
          >
            {copied ? "Copied" : "Copy text"}
          </button>
        </section>
      )}
    </>
  );
}

export function InfoPanel({ asset, onAddTag, onRemoveTag, onNavigate, onRestoreDate, onSetDescription }: Props) {
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
        <Description key={asset.id} value={asset.description ?? ""} onSave={onSetDescription} />
      </section>

      <section>
        <h3>Date</h3>
        <p>{formatTakenAt(asset.takenAt, asset.tzOffsetMin)}</p>
        <p className={uncertainTime ? "warn" : "muted"}>
          {TIME_SOURCES[asset.takenSrc] ?? "unknown source"}
          {asset.takenSrc === 0 && asset.tzOffsetMin !== 0 && ` · ${formatOffset(asset.tzOffsetMin)}`}
        </p>
        {uncertainTime && <p className="muted small">The real capture date may differ: this file had no date inside it.</p>}
        {asset.takenSrc >= 4 && (
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

      <AiInfo id={asset.id} onNavigate={onNavigate} />

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

      {(asset.event || asset.trip) && (
        <section>
          <h3>{asset.event && asset.trip ? "Event and trip" : asset.event ? "Event" : "Trip"}</h3>
          <div className="chips">
            {asset.event && (
              <button className="chip link" onClick={() => onNavigate({ event: asset.event!.id })}>
                {eventTitle(asset.event)}
              </button>
            )}
            {asset.trip && (
              <button className="chip link" onClick={() => onNavigate({ trip: asset.trip!.id })}>
                {tripTitle(asset.trip)}
              </button>
            )}
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
