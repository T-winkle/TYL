// Only mock IPC and a fake API key are used; no live provider requests.
async (page) => {
  await page.setViewportSize({width:960,height:740});
  const check = (ok, label) => { if (!ok) throw Error(label); };
  await page.getByRole('button',{name:'通用',exact:true}).click();
  const proximity = await page.locator('#gpu-current').evaluate(el => ({sameRow:el.closest('.row-text')?.querySelector('#gpu-hint') !== null,gap:el.getBoundingClientRect().top-document.getElementById('gpu-hint').getBoundingClientRect().bottom}));
  check(proximity.sameRow && proximity.gap >= 0 && proximity.gap <= 6,'GPU status belongs directly below its hint');
  await page.locator('#gpu-current').scrollIntoViewIfNeeded();
  await page.screenshot({path:'output/playwright/settings-gpu-inline.png'});
  await page.getByRole('button',{name:'AI 模型',exact:true}).click();
  const key = page.getByLabel('API Key',{exact:true});
  check(await key.getAttribute('type') === 'password','hidden initially');
  await key.fill('fixture-not-a-real-key');
  await page.getByRole('button',{name:'保存更改',exact:true}).click();
  const count = await page.evaluate(()=>window.__tylPreview.calls.length);
  await page.getByRole('button',{name:'显示 API Key',exact:true}).click();
  check(await key.getAttribute('type') === 'text','show plaintext');
  check(await key.inputValue() === 'fixture-not-a-real-key','value unchanged');
  check(await page.getByRole('button',{name:'保存更改',exact:true}).isDisabled(),'toggle does not dirty saved settings');
  check(await page.evaluate(()=>window.__tylPreview.calls.length) === count,'toggle does not send IPC');
  await page.getByRole('button',{name:'隐藏 API Key',exact:true}).click();
  check(await key.getAttribute('type') === 'password','hide again');
  await page.screenshot({path:'output/playwright/settings-key-hidden.png'});
  await page.getByRole('button',{name:'显示 API Key',exact:true}).click();
  await page.getByRole('button',{name:'通用',exact:true}).click();
  await page.getByRole('button',{name:'AI 模型',exact:true}).click();
  check(await key.getAttribute('type') === 'password','hide on tab leave');
  return {passed:['gpu-proximity','key-hidden','key-visible','value-preserved','no-dirty','no-ipc','key-hide','hide-on-leave']};
}
