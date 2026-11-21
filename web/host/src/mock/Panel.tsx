// Development only: picks a scene of src/mock/core.ts. Not part of the design.
import { sceneName, SCENES } from "./core";

export function MockPanel() {
  return (
    <select
      aria-label="Сценарий заглушки"
      title="Сценарий заглушки (только в браузере)"
      style={{ position: "fixed", left: 8, bottom: 1, height: 24, zIndex: 20, width: 280 }}
      value={sceneName}
      onChange={(e) => {
        location.search = `?scene=${e.target.value}`;
      }}
    >
      {Object.entries(SCENES).map(([id, s]) => (
        <option key={id} value={id}>
          {s.title}
        </option>
      ))}
    </select>
  );
}
