import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";

const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  const markdown = await readFile(new URL("README.md", import.meta.url), "utf8");
  const figures = [...markdown.matchAll(/!\[[^\]]*\]\((figures\/[^)]+\.svg)\)/g)];
  assert.ok(figures.length, "Expected SVG figures in the README");

  for (const [, figure] of figures) {
    const svg = await readFile(new URL(figure, import.meta.url));
    const source = "data:image/svg+xml;base64," + svg.toString("base64");
    for (const background of ["#0d1117", "#ffffff"]) {
      const contrast = await page.evaluate(async ({ source, background }) => {
        const img = new Image();
        img.src = source;
        await img.decode();
        const canvas = document.createElement("canvas");
        canvas.width = img.naturalWidth;
        canvas.height = img.naturalHeight;
        const ctx = canvas.getContext("2d");
        ctx.fillStyle = background;
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(img, 0, 0);

        const luminance = ([r, g, b]) => {
          const linear = [r, g, b].map(v => {
            const s = v / 255;
            return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
          });
          return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
        };
        const paper = luminance(ctx.getImageData(2, 2, 1, 1).data);
        // All four charts start with a text heading in the first 38 pixels.
        const { data } = ctx.getImageData(0, 0, canvas.width, 38);
        let best = 1;
        for (let i = 0; i < data.length; i += 4) {
          const ink = luminance(data.subarray(i, i + 3));
          best = Math.max(best, (Math.max(ink, paper) + 0.05) / (Math.min(ink, paper) + 0.05));
        }
        return best;
      }, { source, background });
      assert.ok(contrast >= 4.5, `${figure} on ${background}: ${contrast.toFixed(2)}:1`);
      console.log(`${figure} on ${background}: ${contrast.toFixed(2)}:1`);
    }
  }
  console.log("All README figure headings pass on light and dark backgrounds");
} finally {
  await browser.close();
}
