// PhotoManager landing page: the hero choreography and the small live demos.
// Every demo runs only while it's on screen; with "reduce motion" they show their end state.
(() => {
  "use strict";

  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const SCENES = ["s-sunset", "s-sea", "s-night", "s-forest", "s-desert", "s-snow", "s-meadow", "s-dusk", "s-city", "s-beach"];
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
  const clamp = (v, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));
  const lerp = (a, b, t) => a + (b - a) * t;
  const easeInOut = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
  const easeOut = (t) => 1 - Math.pow(1 - t, 3);

  // A small seeded random, so the pile looks the same on every visit.
  let seed = 20251004;
  const rand = () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };

  const photo = (scene, tag = "i") => {
    const el = document.createElement(tag);
    el.className = `ph ${scene}`;
    return el;
  };

  /** Calls `on(visible)` as `el` enters and leaves the screen. */
  const watch = (el, on, threshold = 0.2) => {
    if (!el) return;
    new IntersectionObserver((entries) => entries.forEach((e) => on(e.isIntersecting)), { threshold }).observe(el);
  };

  // ---------- Nav, year ----------

  const nav = $("#nav");
  const onScrollNav = () => nav.classList.toggle("scrolled", scrollY > 20);
  addEventListener("scroll", onScrollNav, { passive: true });
  onScrollNav();
  $("#year").textContent = new Date().getFullYear();

  // ---------- Reveal on scroll ----------

  const reveals = $$(".reveal");
  if (reduce || !("IntersectionObserver" in window)) {
    reveals.forEach((el) => el.classList.add("in"));
  } else {
    const io = new IntersectionObserver(
      (entries) =>
        entries.forEach((e) => {
          if (e.isIntersecting) {
            e.target.classList.add("in");
            io.unobserve(e.target);
          }
        }),
      { threshold: 0.12, rootMargin: "0px 0px -8% 0px" },
    );
    reveals.forEach((el) => io.observe(el));
  }

  // ---------- Counters ----------

  const countUp = (el) => {
    const to = parseFloat(el.dataset.count);
    const decimals = parseInt(el.dataset.decimals || "0", 10);
    const show = (v) =>
      (el.textContent = v.toLocaleString(undefined, { minimumFractionDigits: decimals, maximumFractionDigits: decimals }));
    if (reduce || to === 0) return show(to);
    const start = performance.now();
    const tick = (now) => {
      const t = clamp((now - start) / 1800);
      show(to * easeOut(t));
      if (t < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  };
  $$("[data-count]").forEach((el) => {
    let done = false;
    watch(el, (v) => {
      if (v && !done) {
        done = true;
        countUp(el);
      }
    }, 0.6);
  });

  // ---------- Hero: the pile becomes a timeline ----------

  const hero = $(".hero");
  const stage = $("#stage");
  const DAYS = [
    { label: "Sat, Oct 4, 2025", n: 6 },
    { label: "Tue, Aug 20, 2024", n: 10 },
    { label: "Sun, Jul 2, 2023", n: 8 },
  ];
  const PILES = ["Phone backup", "D:\\DCIM\\Camera", "WhatsApp Images", "E:\\Old laptop"];
  const total = DAYS.reduce((n, d) => n + d.n, 0);

  const tiles = [];
  for (let i = 0; i < total; i++) {
    const el = photo(SCENES[(i * 7 + 3) % SCENES.length], "div");
    el.classList.add("tile");
    stage.appendChild(el);
    tiles.push({
      el,
      pile: Math.floor(rand() * PILES.length),
      dx: rand() * 2 - 1,
      dy: rand() * 2 - 1,
      rot: (rand() * 2 - 1) * 24,
      scale: 1 + rand() * 0.18,
      z: Math.floor(rand() * 30),
      delay: rand() * 0.3,
    });
  }
  const dayEls = DAYS.map((d) => {
    const el = document.createElement("div");
    el.className = "day";
    el.innerHTML = `${d.label}<small>${d.n} photos</small>`;
    stage.appendChild(el);
    return el;
  });
  const pileEls = PILES.map((name) => {
    const el = document.createElement("div");
    el.className = "pile";
    el.textContent = name;
    stage.appendChild(el);
    return el;
  });

  let geo = null;
  const layoutHero = () => {
    const W = stage.clientWidth;
    const cols = W > 460 ? 6 : 4;
    const gap = 6;
    const head = 30;
    const rows = DAYS.reduce((n, d) => n + Math.ceil(d.n / cols), 0);
    // Tiles fit the width, and while the hero is pinned, the screen's height too.
    let t = (W - gap * (cols - 1)) / cols;
    if (sticky()) {
      const room = innerHeight - 150 - DAYS.length * head - 16 * (DAYS.length - 1);
      t = Math.max(24, Math.min(t, room / rows - gap));
    }
    const ox = (W - (cols * t + (cols - 1) * gap)) / 2;
    let y = 0;
    let k = 0;
    const grid = [];
    const heads = [];
    DAYS.forEach((d) => {
      heads.push(y);
      y += head;
      for (let j = 0; j < d.n; j++) {
        grid[k++] = { x: ox + (j % cols) * (t + gap), y: y + Math.floor(j / cols) * (t + gap) };
      }
      y += Math.ceil(d.n / cols) * (t + gap) + 16;
    });
    const H = y - 16;
    stage.style.height = `${H}px`;
    const centres = [
      [0.22, 0.24],
      [0.74, 0.2],
      [0.28, 0.72],
      [0.76, 0.7],
    ].map(([cx, cy]) => [cx * W, cy * H]);
    tiles.forEach((tile, i) => {
      tile.el.style.width = tile.el.style.height = `${t}px`;
      const [cx, cy] = centres[tile.pile];
      tile.from = { x: cx + tile.dx * W * 0.13 - t / 2, y: cy + tile.dy * H * 0.11 - t / 2 };
      tile.to = grid[i];
    });
    pileEls.forEach((el, i) => {
      el.style.left = `${clamp(centres[i][0] / W - 0.1, 0, 0.7) * W}px`;
      el.style.top = `${centres[i][1] - H * 0.2}px`;
    });
    dayEls.forEach((el, i) => {
      el.style.top = `${heads[i] + 2}px`;
      el.style.left = `${ox}px`;
    });
    geo = { W, H };
  };

  const drawHero = (p) => {
    if (!geo) return;
    tiles.forEach((tile) => {
      const e = easeInOut(clamp((p - tile.delay) / 0.62));
      const x = lerp(tile.from.x, tile.to.x, e);
      const y = lerp(tile.from.y, tile.to.y, e);
      const r = lerp(tile.rot, 0, e);
      const s = lerp(tile.scale, 1, e);
      tile.el.style.transform = `translate3d(${x.toFixed(1)}px, ${y.toFixed(1)}px, 0) rotate(${r.toFixed(2)}deg) scale(${s.toFixed(3)})`;
      tile.el.style.setProperty("--f", (1 - e).toFixed(3));
      tile.el.style.zIndex = e < 0.5 ? tile.z : 1;
    });
    const heads = clamp((p - 0.6) / 0.3);
    dayEls.forEach((el) => {
      el.style.opacity = heads;
      el.style.transform = `translateY(${(1 - heads) * 8}px)`;
    });
    pileEls.forEach((el) => (el.style.opacity = 1 - clamp(p / 0.3)));
    hero.classList.toggle("ordered", p > 0.7);
  };

  const sticky = () => getComputedStyle($(".hero-sticky")).position === "sticky";
  // When the words alone don't fit one screen (a short window, a big zoom), the hero
  // stops pinning: pinned, it would spill over the next section.
  const fitHero = () => {
    hero.classList.remove("loose");
    const copy = $(".hero-copy");
    if (sticky() && copy.offsetHeight + 130 > innerHeight) hero.classList.add("loose");
  };
  let heroP = 0;
  let timed = false;

  const heroFromScroll = () => {
    const rect = hero.getBoundingClientRect();
    const span = rect.height - innerHeight;
    return span > 0 ? clamp(-rect.top / (span * 0.85)) : 1;
  };

  const startHero = () => {
    fitHero();
    layoutHero();
    if (reduce) {
      heroP = 1;
      drawHero(1);
      return;
    }
    if (sticky()) {
      timed = false;
      heroP = heroFromScroll();
      drawHero(heroP);
    } else if (!timed) {
      // No room to scroll the pile into place: it settles by itself once.
      timed = true;
      drawHero(0);
      const start = performance.now() + 700;
      const tick = (now) => {
        heroP = clamp((now - start) / 2600);
        drawHero(heroP);
        if (heroP < 1) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    } else {
      drawHero(heroP);
    }
  };

  let heroQueued = false;
  addEventListener(
    "scroll",
    () => {
      if (heroQueued || reduce || !sticky()) return;
      heroQueued = true;
      requestAnimationFrame(() => {
        heroQueued = false;
        heroP = heroFromScroll();
        drawHero(heroP);
      });
    },
    { passive: true },
  );
  let resizeTimer;
  addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(startHero, 120);
  });
  startHero();

  // ---------- How it works: a scan, on a loop ----------

  const scan = $("#scanDemo");
  if (scan) {
    const steps = $$(".steps li");
    const folders = $$(".folder", scan);
    const label = $(".scan-label", scan);
    const bar = $(".bar", scan);
    const counts = $$("[data-to]", scan);
    const grid = $(".mini-grid", scan);
    const cells = [];
    for (let i = 0; i < 32; i++) {
      const el = photo(SCENES[(i * 3 + 1) % SCENES.length]);
      grid.appendChild(el);
      cells.push(el);
    }
    const TOTAL = 48213;
    const LOOP = 11000;
    let running = false;
    let t0 = 0;

    const setStep = (n) => steps.forEach((li, i) => li.classList.toggle("active", i === n));
    const render = (ms) => {
      if (ms < 1800) {
        const shown = Math.floor(ms / 500);
        folders.forEach((f, i) => f.classList.toggle("on", i < shown));
        folders.forEach((f) => f.classList.remove("busy"));
        label.textContent = "Adding folders…";
        bar.style.setProperty("--p", "0%");
        counts.forEach((c) => (c.textContent = "0"));
        cells.forEach((c) => c.classList.remove("on"));
        setStep(0);
      } else if (ms < 7000) {
        const p = easeInOut((ms - 1800) / 5200);
        folders.forEach((f, i) => {
          f.classList.add("on");
          f.classList.toggle("busy", Math.floor(p * 3) === i);
        });
        label.textContent = `Indexing ${Math.round(p * TOTAL).toLocaleString()} / ${TOTAL.toLocaleString()}`;
        bar.style.setProperty("--p", `${(p * 100).toFixed(1)}%`);
        counts.forEach((c) => (c.textContent = Math.round(p * +c.dataset.to).toLocaleString()));
        cells.forEach((c, i) => c.classList.toggle("on", i < p * cells.length));
        setStep(1);
      } else {
        folders.forEach((f) => f.classList.remove("busy"));
        label.textContent = `Done: ${TOTAL.toLocaleString()} photos and videos, by day`;
        bar.style.setProperty("--p", "100%");
        counts.forEach((c) => (c.textContent = (+c.dataset.to).toLocaleString()));
        cells.forEach((c) => c.classList.add("on"));
        setStep(2);
      }
    };
    const loop = (now) => {
      if (!running) return;
      render((now - t0) % LOOP);
      requestAnimationFrame(loop);
    };
    if (reduce) {
      render(LOOP - 1);
    } else {
      watch(scan, (v) => {
        if (v && !running) {
          running = true;
          t0 = performance.now();
          requestAnimationFrame(loop);
        } else if (!v) {
          running = false;
        }
      });
    }
  }

  // ---------- Timeline: an endless column of days ----------

  const tl = $("#tlScroll");
  if (tl) {
    const days = [
      ["Sat, Oct 4, 2025", 10],
      ["Sun, Sep 14, 2025", 4],
      ["Fri, Jun 6, 2025", 7],
      ["Mon, Mar 31, 2025", 5],
      ["Sat, Dec 21, 2024", 9],
      ["Tue, Aug 20, 2024", 3],
      ["Thu, May 2, 2024", 6],
      ["Sun, Jan 14, 2024", 8],
    ];
    const block = document.createDocumentFragment();
    let k = 0;
    days.forEach(([label, n]) => {
      const head = document.createElement("div");
      head.className = "tl-day";
      head.innerHTML = `${label}<small>${n}</small>`;
      const row = document.createElement("div");
      row.className = "tl-row";
      for (let i = 0; i < n; i++) row.appendChild(photo(SCENES[(k++ * 7) % SCENES.length]));
      block.append(head, row);
    });
    tl.appendChild(block.cloneNode(true));
    tl.appendChild(block);
  }

  // ---------- Editor: before and after ----------

  const compare = $("#compare");
  if (compare) {
    const handle = $("#compareHandle");
    let touched = false;
    let swaying = false;
    const setPos = (pct) => {
      const v = clamp(pct, 2, 98);
      compare.style.setProperty("--pos", `${v}%`);
      handle.setAttribute("aria-valuenow", Math.round(v));
    };
    const fromPointer = (e) => {
      const r = compare.getBoundingClientRect();
      setPos(((e.clientX - r.left) / r.width) * 100);
    };
    compare.addEventListener("pointerdown", (e) => {
      touched = true;
      compare.setPointerCapture(e.pointerId);
      fromPointer(e);
      const move = (ev) => fromPointer(ev);
      const up = () => {
        compare.removeEventListener("pointermove", move);
        compare.removeEventListener("pointerup", up);
      };
      compare.addEventListener("pointermove", move);
      compare.addEventListener("pointerup", up);
    });
    handle.addEventListener("keydown", (e) => {
      const now = parseFloat(handle.getAttribute("aria-valuenow"));
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        touched = true;
        setPos(now + (e.key === "ArrowLeft" ? -5 : 5));
        e.preventDefault();
      }
    });
    setPos(50);
    if (!reduce) {
      const sway = (now) => {
        if (!swaying || touched) return;
        setPos(50 + Math.sin(now / 1100) * 28);
        requestAnimationFrame(sway);
      };
      watch(compare, (v) => {
        swaying = v;
        if (v) requestAnimationFrame(sway);
      });
    }
  }

  // ---------- Collage: layouts take turns ----------

  const collage = $("#collage");
  if (collage && !reduce) {
    const LAYOUTS = [
      ["grid", "Grid"],
      ["mosaic", "Mosaic"],
      ["feature", "Feature"],
      ["strips", "Strips"],
    ];
    const name = $("#collageName");
    let i = 0;
    let timer = null;
    watch(collage, (v) => {
      clearInterval(timer);
      if (!v) return;
      timer = setInterval(() => {
        collage.classList.add("swap");
        setTimeout(() => {
          i = (i + 1) % LAYOUTS.length;
          collage.dataset.layout = LAYOUTS[i][0];
          name.textContent = LAYOUTS[i][1];
          collage.classList.remove("swap");
        }, 360);
      }, 2800);
    });
  }

  // ---------- AI: four demos ----------

  const tabs = $$(".ai-tab");
  const panels = $$(".ai-panel");
  const aiTitle = $("#aiTitle");
  const TITLES = { search: "Search", people: "People", text: "Text in photos", upscale: "Export · Enlarge 4×" };
  const CYCLE = 7000;
  let aiCurrent = "search";
  let aiVisible = false;
  let aiTimer = null;

  // Search: typed descriptions pick out matching photos.
  const searchGrid = $("#searchGrid");
  const typed = $("#typed");
  const searchScenes = ["s-beach", "s-city", "s-snow", "s-sunset", "s-forest", "s-sea", "s-night", "s-meadow", "s-dusk", "s-desert", "s-beach", "s-snow"];
  searchScenes.forEach((s) => searchGrid.appendChild(photo(s)));
  const QUERIES = [
    { q: "beach", match: ["s-beach", "s-sea"] },
    { q: "mountains in the snow", match: ["s-snow", "s-night"] },
    { q: "sunset", match: ["s-sunset", "s-dusk", "s-city"] },
  ];
  let qi = 0;
  let searchRun = 0;
  const runSearch = () => {
    const run = ++searchRun;
    const { q, match } = QUERIES[qi++ % QUERIES.length];
    searchGrid.classList.remove("matching");
    $$(".ph", searchGrid).forEach((p) => p.classList.toggle("match", match.some((m) => p.classList.contains(m))));
    if (reduce) {
      typed.textContent = q;
      searchGrid.classList.add("matching");
      return;
    }
    typed.textContent = "";
    let c = 0;
    const type = () => {
      if (run !== searchRun) return;
      typed.textContent = q.slice(0, ++c);
      if (c < q.length) setTimeout(type, 75);
      else setTimeout(() => run === searchRun && searchGrid.classList.add("matching"), 250);
    };
    setTimeout(type, 300);
  };

  // People: scattered faces gather into people.
  const facesBox = $("#faces");
  const PEOPLE = [
    { name: "Sara", skin: "#f1c7a5", hair: "#3b2a20", cx: 18 },
    { name: "Ali", skin: "#c98e65", hair: "#1b1410", cx: 50 },
    { name: "Add a name", skin: "#e7b896", hair: "#8a5a35", cx: 82 },
  ];
  PEOPLE.forEach((p, pi) => {
    for (let j = 0; j < 4; j++) {
      const f = document.createElement("span");
      f.className = "face";
      const angle = (j / 4) * Math.PI * 2 + pi;
      f.style.cssText = `--skin:${p.skin};--hair:${p.hair};--x0:${(5 + rand() * 80).toFixed(1)}%;--y0:${(4 + rand() * 70).toFixed(1)}%;--x1:calc(${p.cx}% - 23px + ${(Math.cos(angle) * 30).toFixed(1)}px);--y1:calc(38% - 23px + ${(Math.sin(angle) * 30).toFixed(1)}px)`;
      facesBox.appendChild(f);
    }
    const label = document.createElement("span");
    label.className = "face-name";
    label.textContent = p.name;
    label.style.cssText = `--x:${p.cx}%;--y:66%`;
    facesBox.appendChild(label);
  });

  // Upscale: a tiny photo, drawn pixel by pixel.
  const tiny = $("#tiny");
  if (tiny) {
    const ctx = tiny.getContext("2d");
    for (let y = 0; y < 12; y++) {
      for (let x = 0; x < 16; x++) {
        const sky = y / 12;
        let col = `rgb(${255}, ${Math.round(138 + sky * 80)}, ${Math.round(90 + sky * 64)})`;
        if ((x - 11) ** 2 + (y - 3) ** 2 < 4) col = "#fff3c4";
        const hill = 8 + Math.round(Math.sin(x / 2.2) * 1.5);
        if (y >= hill) col = "#7a4b6a";
        ctx.fillStyle = col;
        ctx.fillRect(x, y, 1, 1);
      }
    }
  }

  let peopleTimer = null;
  const showAi = (name, fromUser) => {
    aiCurrent = name;
    tabs.forEach((t) => {
      const on = t.dataset.ai === name;
      t.classList.toggle("on", on);
      t.setAttribute("aria-selected", on);
      if (on) {
        // Restart the tab's timer line.
        t.classList.remove("on");
        void t.offsetWidth;
        t.classList.add("on");
      }
    });
    panels.forEach((p) => p.classList.toggle("on", p.dataset.panel === name));
    aiTitle.textContent = TITLES[name];
    clearInterval(peopleTimer);
    facesBox.classList.remove("grouped");
    if (name === "search") runSearch();
    if (name === "people") {
      if (reduce) facesBox.classList.add("grouped");
      else {
        setTimeout(() => facesBox.classList.add("grouped"), 500);
        peopleTimer = setInterval(() => facesBox.classList.toggle("grouped"), 3200);
      }
    }
    if (fromUser) restartAiCycle();
  };
  const restartAiCycle = () => {
    clearInterval(aiTimer);
    if (reduce || !aiVisible) return;
    document.documentElement.style.setProperty("--cycle", `${CYCLE}ms`);
    aiTimer = setInterval(() => {
      const names = tabs.map((t) => t.dataset.ai);
      showAi(names[(names.indexOf(aiCurrent) + 1) % names.length], false);
    }, CYCLE);
  };
  tabs.forEach((t) => t.addEventListener("click", () => showAi(t.dataset.ai, true)));
  // Arrow keys move between tabs.
  $(".ai-tabs").addEventListener("keydown", (e) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const i = tabs.findIndex((t) => t.dataset.ai === aiCurrent);
    const next = tabs[(i + (e.key === "ArrowDown" ? 1 : tabs.length - 1)) % tabs.length];
    next.focus();
    showAi(next.dataset.ai, true);
    e.preventDefault();
  });
  watch($(".ai-wrap"), (v) => {
    aiVisible = v;
    if (v) {
      showAi(aiCurrent, false);
      restartAiCycle();
    } else {
      clearInterval(aiTimer);
      clearInterval(peopleTimer);
    }
  });
  if (reduce) showAi("search", false);

  // ---------- Formats ----------

  const formats = $("#formats");
  if (formats) {
    const list = ["JPEG", "HEIC", "HEIF", "PNG", "WebP", "GIF", "AVIF", "TIFF", "DNG", "CR2", "CR3", "NEF", "ARW", "ORF", "RW2",
      "RAF", "SRW", "PEF", "MP4", "MOV", "M4V", "3GP", "AVI", "MKV", "MTS", "M2TS", "WebM", "XMP", "Live Photos"];
    const html = list.map((f) => `<span>${f}</span>`).join("");
    formats.innerHTML = html + html;
  }

  // ---------- Themes ----------

  const mock = $("#appMock");
  const amGrid = $("#amGrid");
  if (mock) {
    for (let i = 0; i < 15; i++) amGrid.appendChild(photo(SCENES[(i * 3) % SCENES.length]));
    $$(".swatches button").forEach((b) =>
      b.addEventListener("click", () => {
        mock.dataset.t = b.dataset.t;
        $$(".swatches button").forEach((o) => {
          o.classList.toggle("on", o === b);
          o.setAttribute("aria-pressed", o === b);
        });
      }),
    );
  }
})();
