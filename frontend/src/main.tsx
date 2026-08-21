import React from "react";
import ReactDOM from "react-dom/client";
// Points @monaco-editor/react's loader at the `monaco-editor` package we bundle,
// replacing its built-in cdn.jsdelivr.net default. It lives here, at the app
// entry, and NOT in SqlEditor/CellEditor on purpose: component tests mock
// `@monaco-editor/react` but not `monaco-editor`, so importing the real ESM
// package from a component would drag all of Monaco into jsdom for every test
// that renders an editor. The entry is never imported by a component test.
import "./lib/monaco";
import App from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><App /></React.StrictMode>,
);
