import { useSyncExternalStore } from "react";
import { theme, THEMES } from "../theme";
import { Icon } from "./Icon";
import { Popover } from "./Popover";

/** The toolbar's theme menu: each theme with a preview of its colors. */
export function ThemePicker() {
  const current = useSyncExternalStore(theme.subscribe, theme.get);
  return (
    <Popover label={<Icon name="palette" />} title="Theme" align="right" buttonClass="icon-button">
      {() => (
        <div className="theme-picker" role="radiogroup" aria-label="Theme">
          <h4>Theme</h4>
          {THEMES.map((t) => (
            <button
              key={t.id}
              role="radio"
              aria-checked={t.id === current}
              className={`theme-option${t.id === current ? " on" : ""}`}
              onClick={() => theme.set(t.id)}
            >
              <span className="theme-swatch" aria-hidden="true">
                {t.swatch.map((color, i) => (
                  <span key={i} style={{ background: color }} />
                ))}
              </span>
              <span className="link-label">{t.label}</span>
              {t.id === current && <Icon name="check" />}
            </button>
          ))}
        </div>
      )}
    </Popover>
  );
}
