// Fixed fixtures only; run via playwright-cli run-code --filename.
async (page) => {
  await page.setViewportSize({width: 560, height: 320});
  const check = (ok, message) => { if (!ok) throw Error(message); };
  const present = (id, allowed = true) => page.evaluate(async ({id, allowed}) => {
    await window.__tylPreview.emit('tyl://captured', {
      request_id:id, text:'A complete translation fixture.', source:{kind:'uia'},
      elapsed_ms:0, target_exe:'custom-editor', engines:['bing','youdao','llm'],
      result_display:'tabs', grow_upward:false, theme:'light', color_scheme:'indigo',
      show_source:false, is_word:false, dictionary:false, can_replace:allowed,
      replace_requires_verification:allowed,
    });
  }, {id, allowed});
  const event = (id, service, phase, text = '') => page.evaluate(async payload =>
    window.__tylPreview.emit('tyl://translate', payload), {request_id:id,service,phase,text});
  const tab = name => page.getByRole('tab', {name, exact:true});
  const text = () => page.getByRole('tabpanel').locator('.result-text').textContent();
  const longA = '甲引擎开头\n' + '这是一行较长的译文，滚动之后再切换到其他引擎。\n'.repeat(25) + '甲引擎完整结尾';
  const longB = '乙引擎开头\n' + '第二份长译文必须从顶部显示，不能继承前一页的位置。\n'.repeat(20) + '乙引擎完整结尾';
  await present(800);
  await event(800, 'bing', 'done', longA);
  await event(800, 'youdao', 'done', longB);
  await event(800, 'llm', 'start');
  check(await text() === longA, 'primary result was truncated');
  const scrollA = await page.getByRole('tabpanel').evaluate(el => {
    el.scrollTop = el.scrollHeight; return el.scrollTop;
  });
  check(scrollA > 100, 'fixture must really overflow');
  await tab('有道').click();
  check(await text() === longB, 'second result was truncated');
  check(await page.getByRole('tabpanel').evaluate(el => el.scrollTop) === 0, 'tab inherited previous scroll');
  await page.getByRole('tabpanel').evaluate(el => { el.scrollTop = el.scrollHeight; });
  await tab('必应').click();
  check(await page.getByRole('tabpanel').evaluate(el => el.scrollTop) === 0, 'return tab did not show beginning');
  check(await text() === longA, 'cached primary changed');
  await event(800, 'llm', 'chunk', '流式开头');
  await tab('AI').click();
  check(await text() === '流式开头', 'streamed prefix missing');
  await tab('有道').click();
  await event(800, 'llm', 'chunk', '与完整结尾');
  await event(800, 'llm', 'done', '流式开头与完整结尾');
  await tab('AI').click();
  check(await text() === '流式开头与完整结尾', 'background stream lost text');
  await event(800, 'llm', 'start');
  await event(800, 'llm', 'chunk', '迟到的片段');
  check(await text() === '流式开头与完整结尾', 'late event changed final result');
  check(await page.getByRole('button', {name:'用当前结果替换原文',exact:true}).isEnabled(), 'unknown editor cannot attempt replacement');
  check((await page.getByRole('button', {name:'用当前结果替换原文',exact:true}).getAttribute('title')).includes('先校验'), 'unknown editor hint missing');
  await present(801, false);
  await event(801, 'bing', 'done', '只读译文');
  check(await page.getByRole('button', {name:'用当前结果替换原文',exact:true}).isDisabled(), 'readonly replacement became enabled');
  await event(800, 'bing', 'done', '旧请求');
  check(await text() === '只读译文', 'stale capture replaced current result');
  return {passed:['tab-scroll-reset','full-cached-text','background-stream','late-events','unknown-replacement','readonly-disabled','stale-capture']};
}
