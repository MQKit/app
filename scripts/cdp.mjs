// Dev helper: drives the running app through the WebView2 DevTools protocol.
// Start the app with the debug config (src-tauri/tauri.debug.conf.json), DevTools on port 9333
//   node scripts/cdp.mjs eval "<js expression>"
//   node scripts/cdp.mjs shot <file.png>
//   node scripts/cdp.mjs click "<css selector>" | text "<button text>"
import { writeFileSync } from "node:fs";

const [cmd, arg] = process.argv.slice(2);
const targets = await (await fetch("http://127.0.0.1:9333/json")).json();
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  }
};
await new Promise((r) => (ws.onopen = r));
const send = (method, params = {}) =>
  new Promise((r) => {
    const i = ++id;
    pending.set(i, r);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
const evaluate = async (expr) => {
  const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.result?.exceptionDetails) return "EXCEPTION: " + JSON.stringify(r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails);
  return r.result?.result?.value;
};

if (cmd === "eval") console.log(JSON.stringify(await evaluate(arg), null, 1));
else if (cmd === "shot") {
  const r = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(arg, Buffer.from(r.result.data, "base64"));
  console.log("saved", arg);
} else if (cmd === "click") console.log(await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(arg)}); if(!e) return 'not found'; e.click(); return 'ok'})()`));
else if (cmd === "text")
  console.log(
    await evaluate(
      `(()=>{const e=[...document.querySelectorAll('button,label,a')].find(b=>b.innerText.trim().includes(${JSON.stringify(arg)})); if(!e) return 'not found'; e.click(); return 'ok: '+e.innerText.trim()})()`,
    ),
  );
ws.close();
