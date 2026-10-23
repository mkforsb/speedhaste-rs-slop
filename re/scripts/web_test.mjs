// Drive the web build in headless Chromium: load SPEEDH.JCL, press keys,
// take screenshots.
// usage (copied next to node_modules): cp re/scripts/web_test.mjs tools/pw/ && node tools/pw/web_test.mjs URL JCL OUTDIR "wait:ms,key:Code,down:Code,up:Code,shot:name,..."
// `pad:SPEC` sets a fake standard-mapping gamepad (same SPEC as the headless
// example: `lx=-1/rt=1/SOUTH`, empty = idle). `reload` reloads the page,
// `click:TEXT` clicks a button, `page:NAME` screenshots the whole page.
import { chromium } from 'playwright';
const [url, jcl, out, script] = process.argv.slice(2);
const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
const page = await browser.newPage({ viewport: { width: 1024, height: 768 } });
page.on('console', m => console.log('console:', m.text()));
page.on('pageerror', e => console.log('pageerror:', e.message));
// Fake gamepad: instances of the real prototypes (so wasm-bindgen's
// instanceof checks pass) with own properties shadowing the native getters.
await page.addInitScript(() => {
  const own = (proto, props) => Object.defineProperties(Object.create(proto),
    Object.fromEntries(Object.entries(props).map(([k, v]) => [k, { value: v }])));
  window.__pad = null;
  navigator.getGamepads = () => {
    const p = window.__pad;
    if (!p) return [null, null, null, null];
    const buttons = Array.from({ length: 17 }, (_, i) => {
      const v = p.buttons[i] || 0;
      return own(GamepadButton.prototype, { pressed: v > 0.5, touched: v > 0, value: v });
    });
    return [own(Gamepad.prototype, { id: 'fake', index: 0, connected: true, mapping: 'standard',
      timestamp: performance.now(), axes: p.axes, buttons }), null, null, null];
  };
});
const BUTTONS = { SOUTH: 0, EAST: 1, WEST: 2, NORTH: 3, LB: 4, RB: 5, BACK: 8, START: 9, UP: 12, DOWN: 13, LEFT: 14, RIGHT: 15 };
function padState(spec) {
  const p = { axes: [0, 0, 0, 0], buttons: [] };
  for (const item of spec.split('/').filter(s => s)) {
    const [k, v] = item.split('=');
    if (v === undefined) p.buttons[BUTTONS[k]] = 1;
    else if (k === 'lx') p.axes[0] = +v;
    else if (k === 'ly') p.axes[1] = +v;
    else if (k === 'lt') p.buttons[6] = +v;
    else if (k === 'rt') p.buttons[7] = +v;
  }
  return p;
}
await page.goto(url);
await page.screenshot({ path: `${out}/web_00_loader.png` });
await page.setInputFiles('input[type=file]', jcl);
await page.waitForSelector('canvas', { timeout: 30000 });
for (const cmd of script.split(',')) {
  const [op, arg] = cmd.split(':');
  if (op === 'wait') await page.waitForTimeout(+arg);
  else if (op === 'key') await page.keyboard.press(arg);
  else if (op === 'down') await page.keyboard.down(arg);
  else if (op === 'up') await page.keyboard.up(arg);
  else if (op === 'reload') await page.reload();
  else if (op === 'click') await page.getByRole('button', { name: arg }).click();
  else if (op === 'page') await page.screenshot({ path: `${out}/web_${arg}.png` });
  else if (op === 'pad') await page.evaluate(s => { window.__pad = s; }, padState(arg || ''));
  else if (op === 'shot') await page.locator('canvas').screenshot({ path: `${out}/web_${arg}.png` });
}
await browser.close();
