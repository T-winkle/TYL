// Verify each settings page owns an independent scroll position and menus stay disabled.
async (page) => {
  await page.setViewportSize({width:960,height:420});
  const check = (ok, label) => { if (!ok) throw Error(label); };
  const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => resolve())));
  const scroll = page.locator('.content-scroll');
  await page.getByRole('button',{name:'通用',exact:true}).click();
  const general = await scroll.evaluate(el => { el.scrollTop=el.scrollHeight; return el.scrollTop; });
  check(general > 0,'general page must be scrollable in fixture');
  await page.getByRole('button',{name:'AI 模型',exact:true}).click();
  await settle();
  check(await scroll.evaluate(el => el.scrollTop) === 0,'new page must not inherit previous scroll');
  const llm = await scroll.evaluate(el => { el.scrollTop=Math.min(48,el.scrollHeight-el.clientHeight); return el.scrollTop; });
  check(llm > 0,'AI page must be scrollable in fixture');
  await page.getByRole('button',{name:'网络',exact:true}).click();
  await settle();
  check(await scroll.evaluate(el => el.scrollTop) === 0,'third page starts at its own top');
  await page.getByRole('button',{name:'通用',exact:true}).click();
  await settle();
  check(Math.abs((await scroll.evaluate(el => el.scrollTop))-general) <= 1,'general scroll restored');
  await page.getByRole('button',{name:'AI 模型',exact:true}).click();
  await settle();
  check(Math.abs((await scroll.evaluate(el => el.scrollTop))-llm) <= 1,'AI scroll restored');
  const menu = await page.getByLabel('API Key',{exact:true}).evaluate(el => {
    const event = new MouseEvent('contextmenu',{bubbles:true,cancelable:true,button:2});
    const dispatched = el.dispatchEvent(event);
    return {dispatched,prevented:event.defaultPrevented};
  });
  check(!menu.dispatched && menu.prevented,'settings context menu disabled');
  return {passed:['independent-scroll','restore-general','restore-ai','settings-context-menu']};
}
