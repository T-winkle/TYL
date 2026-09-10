// Run via playwright-cli run-code --filename after opening the popup preview.
async (page) => {
  await page.setViewportSize({width:560,height:420});
  const check = (ok, label) => { if (!ok) throw Error(label); };
  const present = (id, mode = 'tabs') => page.evaluate(async ({id, mode}) => {
    await window.__tylPreview.emit('tyl://captured', {request_id:id,text:'consider',source:{kind:'uia'},elapsed_ms:0,target_exe:'fixture',engines:['bing','youdao','llm'],result_display:mode,grow_upward:false,theme:'light',color_scheme:'indigo',show_source:false,is_word:true,dictionary:true,can_replace:false});
  }, {id,mode});
  const event = (id, phase, text = '') => page.evaluate(async ({id,phase,text}) => window.__tylPreview.emit('tyl://translate',{request_id:id,service:'dict',phase,text}),{id,phase,text});
  const dict = id => event(id,'dict',JSON.stringify({word:'consider',phonetic_uk:null,phonetic_us:null,senses:[{pos:'v.',definitions:['仔细考虑']}]}));
  const requested = id => page.evaluate(id => window.__tylPreview.calls.filter(c => c.command === 'translate_one' && c.args.request_id === id).map(c => c.args.engine),id);
  const engineTab = () => page.getByRole('tab',{name:'引擎译文',exact:true});
  const dictTab = () => page.getByRole('tab',{name:'词典释义',exact:true});
  await present(200);
  check((await requested(200)).length === 0,'no engine while dictionary pending');
  await dict(200);
  check((await requested(200)).length === 0,'no engine after dictionary success');
  await engineTab().click();
  check(JSON.stringify(await requested(200)) === '["bing"]','tabs only request primary');
  await page.getByText('这是切换标签后按需请求的译文。',{exact:true}).waitFor();
  await dictTab().click();
  await engineTab().click();
  check((await requested(200)).length === 1,'switch back must reuse result');
  await present(201,'stacked');
  check((await requested(201)).length === 0,'stacked is also lazy');
  await engineTab().click();
  await engineTab().click();
  check(JSON.stringify(await requested(201)) === '["bing","youdao","llm"]','stacked loads all once');
  await dict(201);
  check(await engineTab().getAttribute('aria-selected') === 'true','late dictionary respects manual choice');
  await event(201,'error','timeout');
  check((await requested(201)).length === 3,'failure after manual switch must not duplicate');
  await present(202);
  await event(202,'error','timeout');
  check(JSON.stringify(await requested(202)) === '["bing"]','failure starts fallback');
  await page.getByText('这是切换标签后按需请求的译文。',{exact:true}).waitFor();
  await present(203,'stacked');
  await event(203,'dict','invalid JSON');
  check((await requested(203)).length === 3,'malformed dictionary falls back in stacked mode');
  await present(204);
  await event(203,'error','timeout');
  check((await requested(204)).length === 0,'stale failure must not start new translation');
  await dict(204);
  return {passed:['lazy-pending','lazy-success','tabs-primary','reuse-result','stacked-all-once','late-dictionary','failure-dedup','failure-fallback','malformed-fallback','stale-failure']};
}
