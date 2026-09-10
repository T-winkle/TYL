// A CPU/layout micro-benchmark of the current off-screen clone measurement.
async (page) => {
  await page.setViewportSize({width:560,height:680});
  const result = await page.evaluate(() => {
    const popup = document.querySelector('.popup');
    if (!popup) throw Error('popup missing');
    const sample = () => {
      const probe = document.createElement('div');
      probe.style.cssText = `position:fixed;left:-10000px;top:0;width:${popup.offsetWidth}px;visibility:hidden;pointer-events:none;contain:layout style`;
      const clone = popup.cloneNode(true);
      clone.classList.add('measurement-probe');
      probe.appendChild(clone);
      document.body.appendChild(probe);
      void clone.getBoundingClientRect().height;
      probe.remove();
    };
    for (let i=0;i<20;i++) sample();
    const times=[];
    for (let i=0;i<250;i++) {
      const start=performance.now();
      sample();
      times.push(performance.now()-start);
    }
    times.sort((a,b)=>a-b);
    const percentile = p => times[Math.floor((times.length-1)*p)];
    return {nodes:popup.querySelectorAll('*').length,meanMs:times.reduce((a,b)=>a+b,0)/times.length,p50Ms:percentile(.5),p95Ms:percentile(.95),p99Ms:percentile(.99),iterations:times.length};
  });
  return result;
}
