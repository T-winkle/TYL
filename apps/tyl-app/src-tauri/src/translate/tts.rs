//! TTS 读音：Windows SAPI SpVoice（本地合成，零网络延迟）。
//! 专用 STA 线程持有 voice 实例（COM 线程亲和）。

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Media::Speech::{ISpVoice, SVSFDefault};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};

/// SpVoice coclass：{96749377-3391-11D2-9EE3-00C04F797396}
const CLSID_SPVOICE: GUID = GUID::from_u128(0x9674_9377_3391_11d2_9ee3_00c0_4f79_7396);

static TTS_TX: OnceLock<Mutex<Option<Sender<String>>>> = OnceLock::new();

/// 初始化 TTS（惰性起 STA 线程）。失败只影响读音按钮，静默降级。
pub fn ensure_started() -> bool {
    let tx_slot = TTS_TX.get_or_init(|| Mutex::new(None));
    let mut guard = tx_slot.lock().unwrap();
    if guard.is_some() {
        return true;
    }
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let spawned = std::thread::Builder::new()
        .name("tyl-tts".into())
        .spawn(move || tts_thread(rx))
        .is_ok();
    if spawned {
        *guard = Some(tx);
        true
    } else {
        false
    }
}

/// 朗读文本（异步，立即返回）。英语单词用 en-US 音色最优；
/// 中文文本 SAPI 也会按系统中文音色朗读。
pub fn speak(text: &str) {
    if !ensure_started() {
        return;
    }
    if let Some(tx) = TTS_TX.get().and_then(|m| m.lock().unwrap().clone()) {
        let _ = tx.send(text.to_string());
    }
}

fn tts_thread(rx: std::sync::mpsc::Receiver<String>) {
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            drain_and_drop(rx);
            return;
        }
        let voice: ISpVoice = match CoCreateInstance(&CLSID_SPVOICE, None, CLSCTX_ALL) {
            Ok(v) => v,
            Err(_) => {
                CoUninitialize();
                drain_and_drop(rx);
                return;
            }
        };
        crate::logger::log_str("[tts] SpVoice 就绪");
        while let Ok(text) = rx.recv() {
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let hr = voice.Speak(PCWSTR(wide.as_ptr()), SVSFDefault.0 as u32, None);
            if hr.is_err() {
                crate::logger::log_str(&format!("[tts] Speak 失败: {hr:?}"));
            }
        }
        let _ = voice.cast::<windows::core::IUnknown>();
        CoUninitialize();
    }
}

fn drain_and_drop(rx: std::sync::mpsc::Receiver<String>) {
    while rx.try_recv().is_ok() {}
}
