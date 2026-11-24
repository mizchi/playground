import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ deviceScaleFactor: 2 });
  const markdown = await readFile(new URL("README.md", import.meta.url), "utf8");
  const figures = [...markdown.matchAll(/!\[[^\]]*\]\((figures\/[^)]+\.fig\.png)\)/g)];
  assert.equal(figures.length, 4, "Expected four opaque PNG figures in the published README");

  for (const [, figure] of figures) {
    const pngFile = new URL(figure, import.meta.url);
    const svg = await readFile(new URL(figure.replace(".fig.png", ".svg"), import.meta.url), "utf8");
    await page.setContent('<style>body { margin: 0; background: white } svg { display: block }</style>' + svg);
    const chart = page.locator("svg");
    if (process.argv.includes("--write")) {
      await chart.screenshot({ path: pngFile.pathname, omitBackground: false });
    }
    const png = await readFile(pngFile);
    const stats = await chart.evaluate(async (svg, source) => {
      const img = new Image();
      img.src = source;
      await img.decode();
      const canvas = document.createElement("canvas");
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      const ctx = canvas.getContext("2d");
      ctx.drawImage(img, 0, 0);
      const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height);
      for (let i = 3; i < data.length; i += 4) {
        if (data[i] !== 255) throw new Error("Published PNG contains transparency");
      }
      const luminance = i => {
        const linear = [...data.subarray(i, i + 3)].map(v => {
          const s = v / 255;
          return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
        });
        return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
      };
      const paper = luminance(0);
      const scale = canvas.width / svg.viewBox.baseVal.width;
      const minimum = selector => Math.min(...[...svg.querySelectorAll(selector)].map(el => {
        const { x, y, width, height } = el.getBBox();
        let best = 1;
        for (let row = Math.floor(y * scale); row <= Math.ceil((y + height) * scale); row++) {
          for (let col = Math.floor(x * scale); col <= Math.ceil((x + width) * scale); col++) {
            const ink = luminance(4 * (row * canvas.width + col));
            best = Math.max(best, (Math.max(ink, paper) + 0.05) / (Math.min(ink, paper) + 0.05));
          }
        }
        return best;
      }));
      return { text: minimum("text"), marks: minimum("circle, polyline, g rect") };
    }, "data:image/png;base64," + png.toString("base64"));
    assert.ok(stats.text >= 4.5 && stats.marks >= 3, figure + " " + JSON.stringify(stats));
    console.log(figure, stats);
  }
  console.log("All four published PNGs are opaque with readable labels and marks");
} finally {
  await browser.close();
}
