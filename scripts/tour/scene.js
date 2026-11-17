// The voiced tour, played in the running debug app through the dev bridge
// (scripts/tour/make.py fills in __PLAN__: each scene's files and how long its line lasts).
// A drawn cursor and drawn drags of files; everything else is the app itself, really
// converting the files. Returns when each scene started (wall clock, ms), to lay the
// voice on the recording.
const d = window.__dev;
const PLAN = __PLAN__;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (test, max = 20000) => {
  const t0 = Date.now();
  while (!test() && Date.now() - t0 < max) await sleep(50);
};
const byText = (sel, text) => [...document.querySelectorAll(sel)].find((e) => e.textContent.trim().startsWith(text));
const ms = (name) => PLAN.scenes[name] ?? 3000;

document.getElementById("demo-layer")?.remove();
const layer = document.createElement("div");
layer.id = "demo-layer";
layer.innerHTML = `<style>
  #demo-layer { position: fixed; inset: 0; pointer-events: none; z-index: 2147483647; }
  #demo-cursor { position: absolute; left: 0; top: 0; will-change: transform; }
  #demo-cursor svg { display: block; filter: drop-shadow(0 1px 1.5px rgba(0,0,0,.35)); }
  #demo-ring { position: absolute; left: 1px; top: 1px; width: 30px; height: 30px; margin: -15px 0 0 -15px; border-radius: 50%;
    background: rgba(31, 102, 217, .28); opacity: 0; }
  #demo-ring.go { animation: demo-ring 420ms ease-out; }
  @keyframes demo-ring { from { transform: scale(.2); opacity: 1; } to { transform: scale(1.15); opacity: 0; } }
  #demo-drag { position: absolute; left: 16px; top: 20px; opacity: 0; transition: opacity 180ms; }
  #demo-drag .card { position: relative; display: flex; align-items: center; gap: 8px; padding: 8px 14px 8px 10px; border-radius: 6px;
    background: rgba(255,255,255,.97); box-shadow: 0 6px 18px rgba(0,0,0,.28); font: 13px "Segoe UI Variable Text", "Segoe UI", sans-serif;
    color: #1b1b1b; white-space: nowrap; }
  #demo-drag .card + .card { position: absolute; left: 6px; top: 6px; z-index: -1; }
  #demo-drag .ti { font-size: 20px; }
  #demo-drag .badge { position: absolute; right: -9px; top: -9px; min-width: 20px; height: 20px; border-radius: 10px; background: #1f66d9;
    color: #fff; font: 600 12px "Segoe UI", sans-serif; display: grid; place-items: center; }
  #demo-end { position: absolute; inset: 0; display: grid; place-items: center; opacity: 0; transition: opacity 500ms;
    color: #fff; text-align: center; font-family: "Segoe UI Variable Display", "Segoe UI", sans-serif;
    background: radial-gradient(420px 320px at 50% 38%, rgba(120, 180, 255, .32), transparent 70%),
      radial-gradient(900px 600px at 85% -10%, rgba(140, 200, 255, .35), transparent 60%),
      radial-gradient(900px 700px at 0% 110%, rgba(5, 20, 70, .55), transparent 60%),
      linear-gradient(160deg, #2563d6 0%, #1a4fb8 45%, #0f357f 100%); }
  #demo-end .card { display: flex; flex-direction: column; align-items: center; }
  #demo-end .card > * { opacity: 0; transform: translateY(10px); transition: opacity 450ms ease-out, transform 450ms ease-out; }
  #demo-end .card > .in { opacity: 1; transform: none; }
  #demo-end img { width: 120px; height: 120px; filter: drop-shadow(0 18px 30px rgba(3, 18, 56, .5)); }
  #demo-end h1 { margin: 30px 0 0; font-size: 68px; line-height: 1; font-weight: 700; letter-spacing: -0.03em; }
  #demo-end .sub { margin: 14px 0 0; font: 400 20px "Segoe UI Variable Text", "Segoe UI", sans-serif; color: rgba(255,255,255,.82); }
  #demo-end .pills { display: flex; gap: 10px; margin-top: 30px; }
  #demo-end .pills span { padding: 7px 16px; border-radius: 999px; font: 600 15px "Segoe UI Variable Text", "Segoe UI", sans-serif;
    background: rgba(255,255,255,.12); border: 1px solid rgba(255,255,255,.22); color: #fff; }
  #demo-end .url { margin-top: 40px; padding: 13px 30px; border-radius: 999px; background: #fff; color: #1a4fb8;
    font: 700 22px "Segoe UI Variable Display", "Segoe UI", sans-serif; letter-spacing: .01em; box-shadow: 0 14px 30px -10px rgba(3, 18, 56, .55); }
</style>
<div id="demo-end"><div class="card"><img src="${PLAN.logo}"><h1>MQKit</h1><p class="sub">Convert files on your own PC</p>
  <div class="pills"><span>Free</span><span>No ads</span><span>No account</span><span>Works offline</span></div><div class="url">mqkit.com</div></div></div>
<div id="demo-cursor">
  <div id="demo-ring"></div>
  <div id="demo-drag"><div class="card"><i class="ti"></i><span class="label"></span><span class="badge"></span></div><div class="card">&nbsp;</div></div>
  <svg width="22" height="30" viewBox="0 0 22 30"><path d="M1.5 1.5 L1.5 23 L6.8 17.9 L10.6 26.6 L14 25.1 L10.3 16.6 L17.6 16.6 Z" fill="#fff" stroke="#000" stroke-width="1.3" stroke-linejoin="round"/></svg>
</div>`;
document.body.appendChild(layer);
const cursor = layer.querySelector("#demo-cursor");
const ring = layer.querySelector("#demo-ring");
const drag = layer.querySelector("#demo-drag");
const end = layer.querySelector("#demo-end");
let pos = { x: 700, y: 560 };
const place = (x, y) => {
  pos = { x, y };
  cursor.style.transition = "none";
  cursor.style.transform = `translate(${x}px, ${y}px)`;
};
const move = async (x, y, t = 750) => {
  pos = { x, y };
  cursor.style.transition = `transform ${t}ms cubic-bezier(.45,.05,.25,1)`;
  cursor.style.transform = `translate(${x}px, ${y}px)`;
  await sleep(t);
};
const target = (el, fx = 0.3, fy = 0.5) => {
  const r = el.getBoundingClientRect();
  return [r.left + r.width * fx, r.top + r.height * fy];
};
const click = async (el) => {
  ring.classList.remove("go");
  void ring.offsetWidth;
  ring.classList.add("go");
  await sleep(120);
  el.click();
};
const typeInto = async (input, text, each = 75) => {
  input.focus();
  for (let i = 1; i <= text.length; i++) {
    input.value = text.slice(0, i);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await sleep(each);
  }
};
const clearInput = (input) => {
  input.value = "";
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
// Files dragged in from Explorer and dropped on the window.
const dropFiles = async (files, icon, color) => {
  const names = files.map((f) => f.split(/[\\/]/).pop());
  drag.querySelector(".ti").className = `ti ti-${icon}`;
  drag.querySelector(".ti").style.color = color;
  drag.querySelector(".label").textContent = names.join(", ");
  drag.querySelector(".badge").textContent = String(files.length);
  drag.querySelector(".badge").style.display = files.length > 1 ? "grid" : "none";
  drag.querySelectorAll(".card")[1].style.display = files.length > 1 ? "flex" : "none";
  await move(-40, 470, 600);
  drag.style.opacity = "1";
  await sleep(200);
  cursor.style.transition = "transform 950ms cubic-bezier(.3,.1,.25,1)";
  cursor.style.transform = "translate(640px, 470px)";
  await sleep(200);
  d.app.dragging = true;
  await sleep(750);
  pos = { x: 640, y: 470 };
  await sleep(350);
  drag.style.opacity = "0";
  d.app.dragging = false;
  d.openWithFiles(await d.api.expand(files));
  await sleep(900);
};
const openCard = async (name) => {
  await until(() => byText(".grid .card", name), 4000);
  const card = byText(".grid .card", name);
  await move(...target(card, 0.35, 0.5), 750);
  await sleep(150);
  await click(card);
};
const startAndWait = async () => {
  await until(() => document.querySelector(".foot button.primary:not(:disabled)"), 15000);
  const btn = document.querySelector(".foot button.primary");
  await move(...target(btn, 0.5, 0.55), 800);
  await sleep(150);
  await click(btn);
  await until(() => document.querySelector(".done"), 60000);
};
const home = async () => {
  d.app.pending = [];
  d.go({ name: "home" });
  await sleep(250);
  document.querySelector(".home")?.scrollTo(0, 0);
};

const marks = [];
// Each scene lasts at least as long as its line, plus a breath.
const scene = async (name, body) => {
  const t0 = Date.now();
  marks.push({ scene: name, at: t0 });
  await body();
  const left = ms(name) + 450 - (Date.now() - t0);
  if (left > 0) await sleep(left);
};

// The starting point: the home screen, as anyone sees it the first time.
for (const id of ["pdf-compress", "heic-to-jpg", "video-to-mp3"]) delete d.app.lastValues[id];
d.app.dragging = false;
await home();
place(760, 520);
await sleep(900);

await scene("intro", async () => {
  // A look down the list of tools and back.
  const list = document.querySelector(".home");
  await move(820, 420, 900);
  for (let y = 0; y <= 900; y += 30) {
    list.scrollTo(0, y);
    await sleep(28);
  }
  await sleep(500);
  list.scrollTo({ top: 0, behavior: "smooth" });
  await sleep(700);
});

await scene("private", async () => {
  const badge = document.querySelector(".privacy");
  await move(...target(badge, 0.55, 0.5), 1100);
});

await scene("search", async () => {
  const input = document.querySelector(".search input");
  await move(...target(input, 0.25, 0.5), 900);
  await click(input);
  await typeInto(input, "make a pdf smaller");
  await sleep(1300);
  clearInput(input);
  await typeInto(input, "ID:12", 110);
  await sleep(1400);
  clearInput(input);
  input.blur();
});

await scene("drop", async () => {
  await dropFiles(PLAN.files.pdf, "file-type-pdf", "#d6452a");
});

await scene("compress", async () => {
  await openCard("Compress PDF");
  await until(() => /about|→/.test(document.querySelector(".foot")?.textContent ?? ""), 15000);
  await sleep(500);
  const chip = byText(".chips .chip", "Smallest");
  if (chip) {
    await move(...target(chip, 0.5, 0.5), 700);
    await click(chip);
    await sleep(900);
  }
  const est = document.querySelector(".foot .est");
  if (est) await move(...target(est, 0.6, 0.5), 900);
});

await scene("result", async () => {
  await startAndWait();
  await sleep(300);
  const safe = document.querySelector(".safe");
  if (safe) await move(...target(safe, 0.3, 0.5), 900);
});

await scene("heic", async () => {
  await home();
  await sleep(300);
  await dropFiles(PLAN.files.heic, "photo", "#0f8a5f");
  await openCard("HEIC to JPG");
  await startAndWait();
});

await scene("mp3", async () => {
  await home();
  await sleep(300);
  await dropFiles(PLAN.files.video, "movie", "#6a5bd9");
  await openCard("Video to MP3");
  await startAndWait();
});

await scene("dark", async () => {
  await home();
  const other = PLAN.theme === "dark" ? "light" : "dark";
  await sleep(500);
  d.app.settings.theme = other;
  d.applySettings();
  await sleep(1600);
  d.app.settings.theme = PLAN.theme;
  d.applySettings();
});

await scene("outro", async () => {
  await move(760, 520, 700);
  cursor.style.opacity = "0";
  end.style.opacity = "1";
  // Logo, name, line, pills, address: one after another.
  await sleep(250);
  for (const el of end.querySelectorAll(".card > *")) {
    el.classList.add("in");
    await sleep(160);
  }
  await sleep(500);
});
await sleep(1500);
const endAt = Date.now();
// The app back as it was, once the recording has stopped (a moment after this returns).
setTimeout(() => layer.remove(), 2500);
return { marks, end: endAt };
