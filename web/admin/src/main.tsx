import { lazy } from "react";
import { App } from "./App";
import { mount } from "../../client/src/boot";
// The look is Lumen Client's: its styles and small parts are used as they are, the admin adds its own.
import "../../client/src/style.css";
import "./style.css";

// `vite --mode mock` only; the real build drops these branches and the files. The stand-in replaces fetch first.
const mock = import.meta.env.MODE === "mock";
const standIn = mock ? import("./mock/core") : Promise.resolve();
const MockPanel = mock ? lazy(() => import("./mock/Panel").then((m) => ({ default: m.MockPanel }))) : null;

standIn.then(() => mount(App, MockPanel));
