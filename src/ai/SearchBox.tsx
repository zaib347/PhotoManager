import { useState } from "react";

type Props = {
  /** Searches; resolves when the results show. */
  onSearch: (query: string) => Promise<void>;
  placeholder?: string;
};

/** "Search your photos" in the toolbar, once search by description or text in photos is installed. */
export function SearchBox({ onSearch, placeholder = "Search: beach, birthday cake…" }: Props) {
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);

  const run = async () => {
    const q = query.trim();
    if (!q || busy) return;
    setBusy(true);
    try {
      await onSearch(q);
    } finally {
      setBusy(false);
    }
  };

  return (
    <input
      className="ai-search"
      type="search"
      placeholder={busy ? "Searching…" : placeholder}
      value={query}
      onChange={(e) => setQuery(e.target.value)}
      onKeyDown={(e) => e.key === "Enter" && run()}
      aria-label="Search photos"
    />
  );
}
