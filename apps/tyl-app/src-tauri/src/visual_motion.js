// Injected only by the opt-in native Release test driver, never shipped in normal builds.
// rAF scheduling is NOT a measurement of compositor-presented frames.
(async () => {
  const popup = document.querySelector('.popup');
  // This machine can request reduced motion. Exercise the normal production
  // transition locally without changing Windows accessibility preferences.
  popup?.style.setProperty('transition-duration', '130ms, 180ms', 'important');
  const scroll = document.querySelector('.translation, .dict, .results-stack, .content-scroll');
  const extent = scroll ? scroll.scrollHeight - scroll.clientHeight : 0;
  for (const phase of ['transition', 'scroll']) {
    const intervals = [];
    let previous, start, visibilityChanged = false;
    await new Promise(resolve => {
      const tick = now => {
        start ??= now;
        if (previous !== undefined) intervals.push(now - previous);
        previous = now;
        visibilityChanged ||= document.visibilityState !== 'visible';
        const elapsed = now - start;
        if (phase === 'transition' && popup) popup.classList.toggle('entered', elapsed % 600 >= 220);
        if (phase === 'scroll' && scroll) scroll.scrollTop = extent * (1 - Math.cos(elapsed / 4000 * Math.PI * 4)) / 2;
        if (elapsed < 4000) requestAnimationFrame(tick); else resolve();
      };
      requestAnimationFrame(tick);
    });
    popup?.classList.add('entered');
    if (scroll) scroll.scrollTop = 0;
    const sorted = [...intervals].sort((a,b) => a-b);
    const percentile = q => Math.round(sorted[Math.min(sorted.length-1, Math.ceil(sorted.length*q)-1)] * 100) / 100;
    await window.__TAURI_INTERNALS__.invoke('frontend_log', {message: '[visual-bench] ' + JSON.stringify({
      scene: window.__tylVisualScene, phase, frames: intervals.length,
      median_ms: percentile(.5), p95_ms: percentile(.95), max_ms: percentile(1),
      over_33ms: intervals.filter(x => x > 33.4).length,
      visibility: document.visibilityState, visibilityChanged,
      reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
      normalMotionForced: true, transitionDuration: popup ? getComputedStyle(popup).transitionDuration : null,
      scrollExtent: extent, viewport: [innerWidth, innerHeight], dpr: devicePixelRatio
    })});
  }
  window.__tylVisualDone = true;
  popup?.style.removeProperty('transition-duration');
})();
