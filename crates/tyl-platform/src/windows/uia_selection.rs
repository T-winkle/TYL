//! Shared, conservative selection checks for capture and replacement. COM thread only.

use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayGetVartype,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_BOOL, VT_EMPTY, VT_I4};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationTextEditPattern, IUIAutomationTextPattern,
    IUIAutomationTextRange, IUIAutomationValuePattern, UIA_EditControlTypeId,
    UIA_IsReadOnlyAttributeId, UIA_TextEditPatternId, UIA_TextPatternId, UIA_ValuePatternId,
};

/// Recover a selection cleared on blur, but only in the focused, positively
/// editable TextPattern provider, with one exact occurrence and no new selection.
/// The caller must bind foreground/native focus before and after this operation.
pub fn restore_unique_selection(
    uia: &IUIAutomation,
    expected: &str,
    identity: Option<&[i32]>,
    still_current: impl Fn() -> bool,
) -> Result<(), String> {
    unsafe {
        let mut owner = uia.GetFocusedElement().map_err(|_| "无法读取焦点控件")?;
        let pid = owner.CurrentProcessId().map_err(|_| "无法确认原编辑器")?;
        for depth in 0..4 {
            if !still_current()
                || owner.CurrentProcessId().ok() != Some(pid)
                || owner.CurrentIsEnabled().ok().map(|v| v.as_bool()) != Some(true)
                || owner.CurrentIsPassword().ok().map(|v| v.as_bool()) != Some(false)
            {
                return Err("无法确认原输入控件".into());
            }
            if let Ok(pattern) =
                owner.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            {
                let ranges = pattern.GetSelection().map_err(|_| "无法读取选区")?;
                for index in 0..ranges.Length().map_err(|_| "无法读取选区")? {
                    if !ranges
                        .GetElement(index)
                        .and_then(|r| r.GetText(-1))
                        .map_err(|_| "无法读取选区")?
                        .to_string()
                        .is_empty()
                    {
                        return Err("已有选区，不自动更改".into());
                    }
                }
                let document = pattern.DocumentRange().map_err(|_| "无法读取编辑范围")?;
                let query = windows::core::BSTR::from(expected);
                let range = document
                    .FindText(&query, false, false)
                    .map_err(|_| "原文已不存在")?;
                let content = document
                    .GetText(100_001)
                    .map_err(|_| "无法检查重复原文")?
                    .to_string();
                if content.chars().count() >= 100_000 || !has_unique_occurrence(&content, expected)
                {
                    return Err("原文出现多次，无法确定原选区".into());
                }
                let candidate = TextSelection {
                    owner,
                    range,
                    text: expected.into(),
                    count: 1,
                };
                if selection_editable(uia, &candidate) != Some(true)
                    || identity
                        .is_some_and(|id| selection_identity(&candidate).as_deref() != Some(id))
                    || candidate.range.GetText(-1).map_err(|_| "无法读取原文")? != expected
                    || !still_current()
                {
                    return Err("无法安全恢复原选区".into());
                }
                return candidate
                    .range
                    .Select()
                    .map_err(|_| "编辑器不支持恢复选区".into());
            }
            if depth < 3 {
                owner = uia
                    .ControlViewWalker()
                    .and_then(|w| w.GetParentElement(&owner))
                    .map_err(|_| "此应用未提供编辑范围")?;
            }
        }
    }
    Err("此应用未提供可恢复的选区".into())
}

fn has_unique_occurrence(content: &str, expected: &str) -> bool {
    let Some(first_char) = expected.chars().next() else {
        return false;
    };
    let Some(start) = content.find(expected) else {
        return false;
    };
    // Include overlapping matches; two occurrences are always ambiguous.
    !content[start + first_char.len_utf8()..].contains(expected)
}

pub struct TextSelection {
    pub owner: IUIAutomationElement,
    pub range: IUIAutomationTextRange,
    pub text: String,
    pub count: i32,
}

/// Probe the focused control itself when text had to be captured through the
/// clipboard. ValuePattern and TextEditPattern are capability signals; class
/// names, localized role strings and process allowlists are intentionally not used.
pub fn focused_control_editable(uia: &IUIAutomation) -> Option<bool> {
    unsafe {
        let mut element = uia.GetFocusedElement().ok()?;
        let pid = element.CurrentProcessId().ok()?;
        let mut walker = None;
        for depth in 0..6 {
            // Reaching the desktop/another provider means the bounded search
            // found no capability, not that the original editor is read-only.
            if element.CurrentProcessId().ok() != Some(pid) {
                return None;
            }
            if element.CurrentIsEnabled().ok().map(|v| v.as_bool()) == Some(false)
                || element.CurrentIsPassword().ok().map(|v| v.as_bool()) == Some(true)
            {
                return Some(false);
            }
            let value_readonly = element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()
                .and_then(|value| value.CurrentIsReadOnly().ok().map(|v| v.as_bool()));
            let text_edit = element
                .GetCurrentPatternAs::<IUIAutomationTextEditPattern>(UIA_TextEditPatternId)
                .is_ok();
            let focused_edit = is_focused_edit_control(&element);
            if let Some(editable) = decide_focused_editable(value_readonly, text_edit, focused_edit)
            {
                return Some(editable);
            }
            if depth == 5 {
                break;
            }
            let walker = match &walker {
                Some(walker) => walker,
                None => walker.insert(uia.ControlViewWalker().ok()?),
            };
            element = walker.GetParentElement(&element).ok()?;
        }
    }
    None
}

fn decide_focused_editable(
    value_readonly: Option<bool>,
    text_edit: bool,
    focused_edit: bool,
) -> Option<bool> {
    value_readonly
        .map(|readonly| !readonly)
        .or((text_edit || focused_edit).then_some(true))
}

/// Some Chromium-derived rich editors (including embedded chat composers) do
/// not expose ValuePattern/TextEditPattern, but do expose the standard Edit
/// control role. Only accept that weaker signal when this exact element owns
/// keyboard focus and is keyboard-focusable; never grant the same fallback to
/// Document controls or unfocused ancestors.
fn is_focused_edit_control(element: &IUIAutomationElement) -> bool {
    unsafe {
        element.CurrentControlType().ok() == Some(UIA_EditControlTypeId)
            && element
                .CurrentHasKeyboardFocus()
                .ok()
                .is_some_and(|value| value.as_bool())
            && element
                .CurrentIsKeyboardFocusable()
                .ok()
                .is_some_and(|value| value.as_bool())
    }
}

/// Some editors focus a child rather than the TextPattern owner. Search only a
/// few ancestors, within the same process, never siblings or the entire desktop.
pub fn focused_selection(uia: &IUIAutomation) -> Result<TextSelection, String> {
    unsafe {
        let mut owner = uia.GetFocusedElement().map_err(|_| "无法读取焦点控件")?;
        let pid = owner.CurrentProcessId().map_err(|_| "无法确认原编辑器")?;
        let mut walker = None;
        for depth in 0..4 {
            if owner.CurrentProcessId().ok() != Some(pid)
                || owner.CurrentIsEnabled().ok().map(|v| v.as_bool()) != Some(true)
                || owner.CurrentIsPassword().ok().map(|v| v.as_bool()) != Some(false)
            {
                return Err("无法确认控件可用，或控件为密码输入框".into());
            }
            if let Ok(pattern) =
                owner.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            {
                let ranges = pattern.GetSelection().map_err(|_| "无法读取选区")?;
                let count = ranges.Length().map_err(|_| "无法读取选区数量")?;
                for index in 0..count {
                    let range = ranges.GetElement(index).map_err(|_| "无法读取选区")?;
                    let text = range
                        .GetText(-1)
                        .map_err(|_| "无法读取选中文本")?
                        .to_string();
                    if !text.trim().is_empty() {
                        return Ok(TextSelection {
                            owner,
                            range,
                            text,
                            count,
                        });
                    }
                }
                // An explicit empty selection must not fall through to a parent's stale selection.
                return Err("没有选中的文本".into());
            }
            if depth == 3 {
                break;
            }
            let walker = match &walker {
                Some(walker) => walker,
                None => walker.insert(uia.ControlViewWalker().map_err(|_| "无法读取编辑器结构")?),
            };
            owner = walker
                .GetParentElement(&owner)
                .map_err(|_| "此应用未提供可验证的选区")?;
        }
    }
    Err("此应用未提供可验证的选区，请复制译文后粘贴".into())
}

/// An explicit read-only flag always wins. Missing range attributes
/// may fall back to ValuePattern, but the role name alone never grants write access.
pub fn selection_editable(uia: &IUIAutomation, selection: &TextSelection) -> Option<bool> {
    if selection.count != 1 {
        return Some(false);
    }
    unsafe {
        let attribute = selection
            .range
            .GetAttributeValue(UIA_IsReadOnlyAttributeId)
            .ok();
        let readonly = attribute.as_ref().and_then(strict_bool);
        if readonly == Some(true) {
            return Some(false);
        }
        if let Some(value) = attribute.as_ref().filter(|_| readonly.is_none()) {
            // Mixed read-only/writable content is not the same as unsupported.
            if value.vt() != VT_EMPTY
                && uia.CheckNotSupported(value).ok().map(|v| v.as_bool()) != Some(true)
            {
                return Some(false);
            }
        }

        let pid = selection.owner.CurrentProcessId().ok()?;
        let mut element = selection.range.GetEnclosingElement().ok()?;
        let mut walker = None;
        for depth in 0..5 {
            if element.CurrentProcessId().ok() != Some(pid)
                || element.CurrentIsEnabled().ok().map(|v| v.as_bool()) == Some(false)
                || element.CurrentIsPassword().ok().map(|v| v.as_bool()) == Some(true)
            {
                return Some(false);
            }
            if let Ok(value) =
                element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            {
                if let Some(editable) = decide_editable(
                    readonly,
                    value.CurrentIsReadOnly().ok().map(|v| v.as_bool()),
                ) {
                    return Some(editable);
                }
            }
            // TextEditPattern is specifically implemented by editable text
            // providers. It is a stronger generic signal than control names,
            // classes or application allowlists when IsReadOnly is missing.
            if element
                .GetCurrentPatternAs::<IUIAutomationTextEditPattern>(UIA_TextEditPatternId)
                .is_ok()
            {
                return Some(true);
            }
            if readonly != Some(true) && is_focused_edit_control(&element) {
                return Some(true);
            }
            if uia
                .CompareElements(&element, &selection.owner)
                .ok()?
                .as_bool()
                || depth == 4
            {
                return decide_editable(readonly, None);
            }
            let walker = match &walker {
                Some(walker) => walker,
                None => walker.insert(uia.ControlViewWalker().ok()?),
            };
            element = walker.GetParentElement(&element).ok()?;
        }
    }
    None
}

fn decide_editable(range_readonly: Option<bool>, value_readonly: Option<bool>) -> Option<bool> {
    if range_readonly == Some(true) || value_readonly == Some(true) {
        Some(false)
    } else {
        range_readonly.or(value_readonly).map(|readonly| !readonly)
    }
}

fn strict_bool(value: &VARIANT) -> Option<bool> {
    // VariantToBoolean would coerce VT_EMPTY / numeric zero to false (writable!).
    (value.vt() == VT_BOOL)
        .then(|| bool::try_from(value).ok())
        .flatten()
}

/// Runtime ID binds replacement to the original selection container, not merely
/// another field with the same text in the same application.
pub fn selection_identity(selection: &TextSelection) -> Option<Vec<i32>> {
    unsafe {
        let element = selection.range.GetEnclosingElement().ok()?;
        let array = OwnedSafeArray(element.GetRuntimeId().ok()?);
        if !array.has_type(VT_I4) {
            return None;
        }
        let (lo, hi) = array.bounds()?;
        let len = i64::from(hi) - i64::from(lo) + 1;
        if !(1..=64).contains(&len) {
            return None;
        }
        let mut result = Vec::with_capacity(len as usize);
        for index in lo..=hi {
            let mut value = 0i32;
            SafeArrayGetElement(array.0, &index, (&mut value as *mut i32).cast()).ok()?;
            result.push(value);
        }
        Some(result)
    }
}

/// UIA out SAFEARRAYs are caller-owned, unlike arrays contained in VARIANTs.
pub(crate) struct OwnedSafeArray(pub *mut SAFEARRAY);

impl OwnedSafeArray {
    pub fn has_type(&self, expected: VARENUM) -> bool {
        !self.0.is_null()
            && unsafe {
                SafeArrayGetDim(self.0) == 1 && SafeArrayGetVartype(self.0).ok() == Some(expected)
            }
    }

    pub fn bounds(&self) -> Option<(i32, i32)> {
        if self.0.is_null() {
            return None;
        }
        unsafe {
            Some((
                SafeArrayGetLBound(self.0, 1).ok()?,
                SafeArrayGetUBound(self.0, 1).ok()?,
            ))
        }
    }
}

impl Drop for OwnedSafeArray {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = SafeArrayDestroy(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_recovery_requires_one_exact_occurrence() {
        assert!(has_unique_occurrence("before selected after", "selected"));
        assert!(has_unique_occurrence("前文中文后文", "中文"));
        assert!(!has_unique_occurrence("aaaa", "aaa"));
        assert!(!has_unique_occurrence("中文中文", "中文"));
        assert!(!has_unique_occurrence("changed", "selected"));
        assert!(!has_unique_occurrence("anything", ""));
    }

    #[test]
    fn readonly_and_unknown_never_enable_replacement() {
        assert_eq!(decide_editable(Some(true), Some(false)), Some(false));
        assert_eq!(decide_editable(Some(false), Some(true)), Some(false));
        assert_eq!(decide_editable(None, Some(true)), Some(false));
        assert_eq!(decide_editable(None, None), None);
    }

    #[test]
    fn accepts_explicit_range_or_value_writability() {
        assert_eq!(decide_editable(Some(false), None), Some(true));
        assert_eq!(decide_editable(None, Some(false)), Some(true));
    }

    #[test]
    fn never_coerces_missing_or_non_boolean_attributes_to_writable() {
        assert_eq!(strict_bool(&VARIANT::default()), None);
        assert_eq!(strict_bool(&VARIANT::from(0i32)), None);
        assert_eq!(strict_bool(&VARIANT::from(1i32)), None);
        assert_eq!(strict_bool(&VARIANT::from(false)), Some(false));
        assert_eq!(strict_bool(&VARIANT::from(true)), Some(true));
    }

    #[test]
    fn focused_editor_requires_a_positive_capability_signal() {
        assert_eq!(
            decide_focused_editable(Some(false), false, false),
            Some(true)
        );
        assert_eq!(decide_focused_editable(Some(true), true, true), Some(false));
        assert_eq!(decide_focused_editable(None, true, false), Some(true));
        assert_eq!(decide_focused_editable(None, false, true), Some(true));
        assert_eq!(decide_focused_editable(None, false, false), None);
    }
}
