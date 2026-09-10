// Read-only pixel comparison of native CapturePreview PNGs (not desktop FPS).
// node scripts/compare-visual-captures.cjs <sharp-module-path> <off-dir> <on-dir>
const sharp = require(process.argv[2]);
const path = require('node:path');
(async () => {
  const results = [];
  for (const scene of ['light-longtext', 'dark-longtext', 'light-dictionary', 'dark-dictionary', 'settings-light', 'settings-dark']) {
    const a = await sharp(path.join(process.argv[3], `visual-off-${scene}.png`)).ensureAlpha().raw().toBuffer({resolveWithObject:true});
    const b = await sharp(path.join(process.argv[4], `visual-on-${scene}.png`)).ensureAlpha().raw().toBuffer({resolveWithObject:true});
    if (a.info.width !== b.info.width || a.info.height !== b.info.height) throw Error(`Size mismatch: ${scene}`);
    let different = 0, total = 0, peak = 0, alphaDifference = 0;
    for (let i=0; i<a.data.length; i+=4) {
      let delta = 0;
      for (let c=0; c<4; c++) { const d = Math.abs(a.data[i+c]-b.data[i+c]); delta = Math.max(delta,d); total += d; }
      if (delta > 0) different++;
      if (a.data[i+3] !== b.data[i+3]) alphaDifference++;
      peak = Math.max(peak,delta);
    }
    results.push({scene, width:a.info.width, height:a.info.height,
      differentPixelsPct: different/(a.info.width*a.info.height)*100,
      meanAbsoluteChannelDifference:total/a.data.length, peakChannelDifference:peak, alphaDifference});
  }
  console.log(JSON.stringify(results,null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
