import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { Overlay } from "./Overlay";
import "./overlay.css";

// `vite --mode mock` only; the real build drops this branch and the file.
const MockPanel =
  import.meta.env.MODE === "mock" ? lazy(() => import("../mock/OverlayPanel").then((m) => ({ default: m.OverlayPanel }))) : null;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Overlay />
    {MockPanel && (
      <Suspense>
        <MockPanel />
      </Suspense>
    )}
  </StrictMode>,
);
