# PhotoManager website

The landing page: plain HTML, CSS and JavaScript, with no build step and no requests
to other sites (no fonts, trackers or CDNs).

- `index.html`: the page and all its text.
- `styles.css`: layout, the painted "photos" (CSS gradients, like the app's splash) and
  the animations.
- `script.js`: the scroll-linked hero (a pile of photos settles into a timeline) and the
  live demos. Each demo runs only while it's on screen.
- `assets/mark.svg`: the logo and favicon.

Open `index.html` in a browser to view it. With "reduce motion" turned on in Windows
(Settings → Accessibility → Visual effects → Animation effects), every demo shows its
finished state instead of moving.

To publish, upload the folder as it is to any static host (GitHub Pages, Netlify,
Cloudflare Pages). GitHub Pages only serves a branch's root or `/docs` folder, so
either deploy this folder with a Pages workflow (`actions/upload-pages-artifact` with
`path: website`) or copy its contents to the root of a `gh-pages` branch.

Keep the claims true: every feature on the page exists in the app. Update the page when
features, sizes (the AI downloads) or platforms change, and when there's a real download
to link to.
