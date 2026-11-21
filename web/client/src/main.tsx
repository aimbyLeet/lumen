import { lazy } from "react";
import { App } from "./App";
import { mount } from "./boot";
import "./style.css";

// `vite --mode mock` only; the real build drops this branch and the file.
const MockPanel =
  import.meta.env.MODE === "mock" ? lazy(() => import("./mock/Panel").then((m) => ({ default: m.MockPanel }))) : null;

mount(App, MockPanel);
