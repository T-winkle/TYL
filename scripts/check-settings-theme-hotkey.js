// Run against the settings preview; no external services are used.
async (page) => {
  await page.setViewportSize({width:960,height:740});
  const check = (ok, label) => { if (!ok) throw Error(label); };
  await page.getByRole('button',{name:'通用',exact:true}).click();
  check(await page.getByText('主题色',{exact:true}).isVisible(),'theme color label');
  check(await page.getByText('品牌配色',{exact:true}).count() === 0,'old palette label removed');
  check(await page.getByRole('button',{name:'Alt+T',exact:true}).isVisible(),'default hotkey title case');
  const select = page.getByLabel('日志等级');
  await select.scrollIntoViewIfNeeded();
  const geometry = await page.locator('.log-level-wrap').evaluate((wrap) => {
    const select = wrap.querySelector('select');
    const icon = wrap.querySelector('.log-level-chevron');
    const wr = wrap.getBoundingClientRect();
    const ir = icon.getBoundingClientRect();
    const style = getComputedStyle(select);
    return {rightGap:wr.right-ir.right,appearance:style.appearance,paddingRight:parseFloat(style.paddingRight)};
  });
  check(geometry.rightGap >= 9 && geometry.rightGap <= 13,'select icon inset');
  check(geometry.appearance === 'none' && geometry.paddingRight >= 34,'custom select arrow spacing');
  await page.screenshot({path:'output/playwright/settings-theme-hotkey-select.png'});
  return {passed:['theme-color-label','default-alt-t','modifier-title-case','select-arrow-inset']};
}
