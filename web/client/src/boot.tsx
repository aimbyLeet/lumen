import { StrictMode, Suspense, type ComponentType } from "react";
import { createRoot } from "react-dom/client";

/**
 * Mounts the app into #root, with the mock panel beside it when there is one. The panel is loaded by each
 * `main.tsx` itself (`vite --mode mock` only), so the real build drops the branch and the file.
 */
export function mount(App: ComponentType, MockPanel: ComponentType | null) {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
      {MockPanel && (
        <Suspense>
          <MockPanel />
        </Suspense>
      )}
    </StrictMode>,
  );
}
