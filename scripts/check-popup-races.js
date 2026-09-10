// Run with playwright-cli run-code after opening localhost preview.html?case=dict.
async (page) => {
  await page.setViewportSize({width:560,height:420});
  const check = async (condition, label) => { if (!(await condition())) throw Error(label); };
  const present = async (id, dictionary = true) => page.evaluate(async ({id, dictionary}) => {
    await window.__tylPreview.emit('tyl://captured', {request_id:id, text:'consider',source:{kind:'uia'},elapsed_ms:0,target_exe:'fixture',engines:['bing'],result_display:'tabs',grow_upward:false,theme:'light',color_scheme:'indigo',show_source:false,is_word:true,dictionary,can_replace:false});
    await window.__tylPreview.emit('tyl://translate',{request_id:id,service:'bing',phase:'done',text:'普通引擎提前返回'});
  }, {id, dictionary});
  const dictionary = async id => page.evaluate(async id => window.__tylPreview.emit('tyl://translate',{request_id:id,service:'dict',phase:'dict',text:JSON.stringify({word:'consider',phonetic_uk:'kənˈsɪdə',phonetic_us:'kənˈsɪdər',senses:[{pos:'v.',definitions:['仔细考虑']}],exam_types:[]})}),id);
  await present(100);
  await check(()=>page.getByRole('status',{name:'正在查询词典'}).isVisible(),'word must show dictionary loading first');
  await check(async()=>!(await page.getByText('普通引擎提前返回',{exact:true}).isVisible()),'engine result must not flash before dictionary');
  await page.screenshot({path:'output/playwright/dictionary-loading.png'});
  await dictionary(100);
  await check(()=>page.getByText('仔细考虑',{exact:true}).isVisible(),'dictionary result missing');
  await check(async()=>await page.getByRole('tab',{name:'词典释义',exact:true}).getAttribute('aria-selected')==='true','dictionary tab should stay selected');
  await present(101);
  await page.getByRole('tab',{name:'引擎译文',exact:true}).click();
  await dictionary(101);
  await check(()=>page.getByText('普通引擎提前返回',{exact:true}).isVisible(),'late dictionary must not override user tab choice');
  await present(102);
  await page.evaluate(async()=>window.__tylPreview.emit('tyl://translate',{request_id:102,service:'dict',phase:'error',text:'timeout'}));
  await check(()=>page.getByText('普通引擎提前返回',{exact:true}).isVisible(),'dictionary error should release fallback');
  await present(103,false);
  await check(()=>page.getByText('普通引擎提前返回',{exact:true}).isVisible(),'disabled dictionary must not stay loading');
  await present(104);
  await dictionary(103);
  await check(()=>page.getByRole('status',{name:'正在查询词典'}).isVisible(),'stale dictionary should not fill new request');
  await dictionary(104);
  await page.screenshot({path:'output/playwright/dictionary-result.png'});
  return {passed: ['engine-first','dictionary-success','explicit-tab','failure-fallback','disabled-dictionary','stale-request']};
}
