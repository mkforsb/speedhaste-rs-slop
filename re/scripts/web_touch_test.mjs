// Drive the web build's touch controls in headless Chromium on a phone-sized
// touch viewport: plays the built-in shareware, screenshots the overlay.
// usage: cp re/scripts/web_touch_test.mjs tools/pw/ && node tools/pw/web_touch_test.mjs URL OUTDIR "STEPS"
// STEPS, comma-separated: `wait:ms`, `click:TEXT` (a page button),
// `tap:SELECTOR` (touch tap in the middle of an element), `hold:SELECTOR`
// / `release:SELECTOR` (a finger stays down; several can be held),
// `move:HELD=TARGET` (slides the finger held on HELD to TARGET's middle),
// `steer:DX` (a finger down on the left half, slid DX px; `steer:off` lifts),
// `shot:NAME` (page screenshot), `type:TEXT` (into the focused field).
// Env CHROMIUM: browser path (default /usr/bin/chromium); VIEWPORT: WxH in
// CSS px (default 844x390, a phone held sideways; 390x844 upright).
import { chromium } from 'playwright';
const [url, out, script] = process.argv.slice(2);
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || '/usr/bin/chromium',
  args: ['--autoplay-policy=no-user-gesture-required'] });
const [vw, vh] = (process.env.VIEWPORT || '844x390').split('x').map(Number);
const ctx = await browser.newContext({ viewport: { width: vw, height: vh }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });
const page = await ctx.newPage();
page.on('console', m => console.log('console:', m.text()));
page.on('pageerror', e => console.log('pageerror:', e.message));
const cdp = await ctx.newCDPSession(page);
const fingers = new Map(); // id -> {x, y}
let nextId = 1;
const send = type => cdp.send('Input.dispatchTouchEvent', { type,
  touchPoints: [...fingers.entries()].map(([id, p]) => ({ x: p.x, y: p.y, id })) });
// CDP's touchEnd lifts every finger: put the remaining ones back down.
const lift = async id => {
  fingers.delete(id);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  if (fingers.size) await send('touchStart');
};
const center = async sel => {
  const b = await page.locator(sel).first().boundingBox();
  if (!b) throw new Error(`no ${sel}`);
  return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
};
const held = new Map(); // selector -> finger id
let steerId = null;
await page.goto(url);
for (const cmd of script.split(',')) {
  const i = cmd.indexOf(':');
  const [op, arg] = i < 0 ? [cmd, ''] : [cmd.slice(0, i), cmd.slice(i + 1)];
  if (op === 'wait') await page.waitForTimeout(+arg);
  else if (op === 'click') await page.getByRole('button', { name: arg }).click();
  else if (op === 'shot') await page.screenshot({ path: `${out}/touch_${arg}.png` });
  else if (op === 'type') await page.keyboard.type(arg);
  else if (op === 'tap') {
    const id = nextId++;
    fingers.set(id, await center(arg)); await send('touchStart');
    await page.waitForTimeout(30);
    await lift(id);
  } else if (op === 'hold') {
    const id = nextId++; held.set(arg, id);
    fingers.set(id, await center(arg)); await send('touchStart');
  } else if (op === 'move') {
    const [from, to] = arg.split('=');
    fingers.set(held.get(from), await center(to)); await send('touchMove');
  } else if (op === 'release' || (op === 'steer' && arg === 'off')) {
    const id = op === 'release' ? held.get(arg) : steerId;
    await lift(id);
    if (op === 'steer') steerId = null;
  } else if (op === 'steer') {
    if (steerId === null) {
      steerId = nextId++; fingers.set(steerId, { x: 100, y: vh - 110 }); await send('touchStart');
    }
    fingers.set(steerId, { x: 100 + +arg, y: vh - 110 }); await send('touchMove');
  }
}
await browser.close();
