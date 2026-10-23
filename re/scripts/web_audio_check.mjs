// Check web audio output: counts the chunks the page posts to the AudioWorklet,
// their pacing (frames/s should match the sample rate) and loudness.
// usage: cp re/scripts/web_audio_check.mjs tools/pw/ && node tools/pw/web_audio_check.mjs URL JCL
import { chromium } from 'playwright';
const [url, jcl] = process.argv.slice(2);
const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
const page = await browser.newPage();
page.on('console', m => { if (!m.text().startsWith('[')) console.log('console:', m.text()); });
await page.addInitScript(() => {
  window.__chunks = [];
  const orig = MessagePort.prototype.postMessage;
  MessagePort.prototype.postMessage = function (d, t) {
    if (d instanceof Float32Array) {
      let s = 0; for (const v of d) s += v * v;
      window.__chunks.push([performance.now(), d.length / 2, Math.sqrt(s / d.length)]);
    }
    return orig.call(this, d, t);
  };
});
await page.goto(url);
await page.setInputFiles('input[type=file]', jcl);
await page.waitForSelector('canvas');
await page.waitForTimeout(8000);
const c = await page.evaluate(() => window.__chunks);
const frames = c.reduce((a, x) => a + x[1], 0);
const span = (c[c.length - 1][0] - c[0][0]) / 1000;
console.log('chunks', c.length, 'frames', frames, 'span s', span.toFixed(2), 'frames/s', (frames / span).toFixed(0));
console.log('rms over time', c.filter((_, i) => i % 40 == 0).map(x => x[2].toFixed(3)).join(' '));
console.log('rate', await page.evaluate(() => new AudioContext().sampleRate));
await browser.close();
