import React from "react";
import ReactDOM from "react-dom/client";
import { api } from "./api";
import App from "./App";
import { photoTurns } from "./turns";

// Turned photos first: their image URLs differ, and an image fetched under the wrong
// URL would stay cached the wrong way round.
api
  .turnedPhotos()
  .then((turned) => photoTurns.set(turned))
  .catch((e) => console.error("Couldn't load turned photos", e))
  .finally(() =>
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    ),
  );
