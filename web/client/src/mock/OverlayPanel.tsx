// Development only: picks a scene of event.ts, and stands for the video under the overlay. Not part of the design.
import { overlaySceneName, OVERLAY_SCENES } from "./event";

export function OverlayPanel() {
  return (
    <>
      <div style={{ position: "fixed", inset: 0, zIndex: -1, background: "linear-gradient(120deg, #1b4d6b, #7a5c3a 45%, #33563f 70%, #6b2f4d)" }} />
      <select
        aria-label="Сценарий заглушки"
        style={{ position: "fixed", left: 8, top: 8, height: 26, zIndex: 20, width: 300 }}
        value={overlaySceneName}
        onChange={(e) => {
          location.search = `?scene=${e.target.value}`;
        }}
      >
        {Object.entries(OVERLAY_SCENES).map(([id, s]) => (
          <option key={id} value={id}>
            {s.title}
          </option>
        ))}
      </select>
    </>
  );
}
