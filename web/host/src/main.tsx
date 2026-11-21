import { lazy } from "react";
import { App } from "./App";
import { mount } from "../../client/src/boot";
// The look is Lumen Client's: its styles and small parts are used as they are, this app adds its own.
import "../../client/src/style.css";
import "./style.css";

// `vite --mode mock` only; the real build drops this branch and the file.
const MockPanel =
  import.meta.env.MODE === "mock" ? lazy(() => import("./mock/Panel").then((m) => ({ default: m.MockPanel }))) : null;

mount(App, MockPanel);
