import { useState } from "react";
import type { TagInfo } from "../api";

type Props = {
  tags: TagInfo[];
  /** Tags to leave out of suggestions (e.g. already on the photo). */
  exclude?: Set<number>;
  onAdd: (name: string) => void;
  autoFocus?: boolean;
};

/** Text input that adds a tag on Enter, suggesting existing tags as you type. */
export function TagInput({ tags, exclude, onAdd, autoFocus }: Props) {
  const [value, setValue] = useState("");
  const q = value.trim().toLowerCase();
  const suggestions = q
    ? tags.filter((t) => !exclude?.has(t.id) && t.name.toLowerCase().includes(q)).slice(0, 6)
    : [];

  const add = (name: string) => {
    if (!name.trim()) return;
    onAdd(name.trim());
    setValue("");
  };

  return (
    <div className="tag-input">
      <input
        autoFocus={autoFocus}
        placeholder="Add a tag"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") add(value);
        }}
      />
      {suggestions.length > 0 && (
        <ul className="picker-list">
          {suggestions.map((t) => (
            <li key={t.id}>
              <button className="picker-item" onClick={() => add(t.name)}>
                <span>{t.name}</span>
                <span className="muted">{t.count.toLocaleString()}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
