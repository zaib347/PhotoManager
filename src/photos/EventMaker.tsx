import { useState } from "react";

type Props = {
  count: number;
  onCreate: (name: string) => void;
  /** Present when one item inside an event is selected. */
  onSplit?: () => void;
};

/** Make an event from the selection, or split the current event at the selected item. */
export function EventMaker({ count, onCreate, onSplit }: Props) {
  const [name, setName] = useState("");
  return (
    <div className="event-maker">
      {onSplit && (
        <>
          <button onClick={onSplit}>Split event here</button>
          <p className="muted small">This item and everything after it become a new event.</p>
          <hr />
        </>
      )}
      <input
        autoFocus
        placeholder="Event name (optional)"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && onCreate(name)}
      />
      <button className="primary" onClick={() => onCreate(name)}>
        New event from {count.toLocaleString()} {count === 1 ? "item" : "items"}
      </button>
      <p className="muted small">They leave any event they were in.</p>
    </div>
  );
}
