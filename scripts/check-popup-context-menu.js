// Verify popup content does not open the browser/WebView context menu.
async (page) => {
  const result = await page.locator('.popup').evaluate(el => {
    const event = new MouseEvent('contextmenu',{bubbles:true,cancelable:true,button:2});
    const dispatched = el.dispatchEvent(event);
    return {dispatched,prevented:event.defaultPrevented};
  });
  if (result.dispatched || !result.prevented) throw Error('popup context menu enabled');
  return {passed:['popup-context-menu']};
}
