// Open the settings preview; all IPC calls are mocked and the key is a fixture.
async (page) => {
  await page.setViewportSize({width:960,height:740});
  const check = (ok, message) => { if (!ok) throw Error(message); };
  await page.getByRole('button',{name:'通用',exact:true}).click();
  await page.getByLabel('日志等级').selectOption('trace');
  await page.getByRole('button',{name:'保存更改',exact:true}).click();
  check(await page.evaluate(()=>window.__tylPreview.calls.filter(c=>c.command==='save_settings').at(-1)?.args.settings.log_level==='trace'),'log level save payload');
  await page.getByLabel('日志等级').scrollIntoViewIfNeeded();
  await page.screenshot({path:'output/playwright/settings-log-level.png'});
  await page.getByRole('button',{name:'AI 模型',exact:true}).click();
  check(await page.getByRole('button',{name:'测试连接',exact:true}).isDisabled(),'empty key should disable test');
  await page.getByLabel('API Key',{exact:true}).fill('fixture-key-not-a-real-secret');
  const before = await page.evaluate(()=>window.__tylPreview.calls.filter(c=>c.command==='save_settings').length);
  await page.getByRole('button',{name:'测试连接',exact:true}).click();
  const result = page.locator('.connection-result');
  await result.waitFor({state:'visible'});
  const isError = page.url().includes('testError=true');
  check((await result.textContent()).includes(isError?'401':'连接成功'),'test feedback');
  check(await page.evaluate(()=>window.__tylPreview.calls.filter(c=>c.command==='save_settings').length)===before,'testing must not save settings');
  await page.screenshot({path:`output/playwright/settings-ai-${isError?'error':'success'}.png`});
  await page.getByLabel('模型',{exact:true}).fill('another-model');
  check(await result.count()===0,'editing should clear stale connection result');
  return {passed:['log-level-save','empty-key-disabled','test-feedback','test-does-not-save','clear-stale-feedback'], mode:isError?'error':'success'};
}
