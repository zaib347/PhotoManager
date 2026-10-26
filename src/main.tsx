import React from "react";
import ReactDOM from "react-dom/client";
import { api } from "./api";
import App from "./App";
import { photoLooks } from "./looks";
// Applies the saved theme before the first paint.
import "./theme";

// Turned and edited photos first: their image URLs differ, and an image fetched under
// the wrong URL would stay cached with its old look.
api
  .adjustedPhotos()
  .then((list) => photoLooks.load(list))
  .catch((e) => console.error("Couldn't load edited photos", e))
  .finally(() =>
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    ),
  );
