// Run against the settings preview; no external services are used.
async (page) => {
  await page.setViewportSize({width:960,height:740});
  const check = (ok, label) => { if (!ok) throw Error(label); };
  await page.getByRole('button',{name:'通用',exact:true}).click();
  check(await page.getByText('主题色',{exact:true}).isVisible(),'theme color label');
  check(await page.getByText('品牌配色',{exact:true}).count() === 0,'old palette label removed');
  check(await page.getByRole('button',{name:'Alt+T',exact:true}).isVisible(),'default hotkey title case');
  const shortcut = page.getByRole('radio', {name:'快捷键翻译', exact:true});
  const automatic = page.getByRole('radio', {name:'划词自动翻译', exact:true});
  check(await shortcut.getAttribute('aria-checked') === 'true', 'existing settings default to shortcut mode');
  await automatic.click();
  check(await shortcut.getAttribute('aria-checked') === 'false', 'triggers are mutually exclusive');
  check(await page.getByText('触发组合键', {exact:true}).count() === 0, 'shortcut recorder hidden in automatic mode');
  await page.getByRole('button', {name:'保存更改', exact:true}).click();
  await page.waitForFunction(() => window.__tylPreview.calls.some(({command,args}) =>
    command === 'save_settings' && args.settings.trigger_mode === 'automatic' && args.settings.hotkey === 'alt+t'));
  await page.evaluate(() => window.__tylPreview.emit('tyl://trigger-mode', 'hotkey'));
  await page.getByText('触发组合键', {exact:true}).waitFor();
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
  return {passed:['theme-color-label','default-alt-t','exclusive-trigger-modes','tray-mode-sync','modifier-title-case','select-arrow-inset']};
}
