import { useEffect, useState } from "react";
import { api, type XmpSettings as Settings } from "../api";

/**
 * XMP sidecars: always read on scan; writing them (next to the photos) is a choice,
 * off by default since it adds files to the user's folders.
 */
export function XmpSettings({ onError }: { onError: (e: unknown) => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    api.xmpSettings().then(setSettings).catch(onError);
  }, [onError]);

  if (!settings) return null;
  const save = (next: Settings) => {
    setSettings(next);
    api.setXmpSettings(next).catch(onError);
  };
  return (
    <div className="xmp-settings">
      <label className="xmp-write">
        <input type="checkbox" checked={settings.write} onChange={(e) => save({ ...settings, write: e.target.checked })} />
        Write ratings, tags and descriptions to XMP sidecars
      </label>
      {settings.write && (
        <label className="xmp-favorite">
          Favorites as
          <select
            value={settings.favoriteAs}
            onChange={(e) => save({ ...settings, favoriteAs: e.target.value as Settings["favoriteAs"] })}
          >
            <option value="rating">5 stars</option>
            <option value="label">the label “Favorite”</option>
          </select>
        </label>
      )}
      <p className="muted small">
        Sidecars from Lightroom, darktable and digiKam are read on every scan. Writing adds a .xmp file next to each
        photo; the photos themselves are never changed.
      </p>
    </div>
  );
}
