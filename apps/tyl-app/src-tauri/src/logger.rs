//! 极简文件日志：写入 exe 同目录的 tyl.log（目录只读时退回临时目录）。
//! release 构建是 windows_subsystem="windows"，没有控制台，
//! eprintln 全部不可见——这是唯一的诊断通道。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG: Mutex<Option<File>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Level {
    Off = 0,
    Error = 1,
    Warn = 2,
    #[default]
    Info = 3,
    Debug = 4,
    Trace = 5,
}
static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}
fn allows(configured: u8, message: Level) -> bool {
    message != Level::Off && message as u8 <= configured
}
pub fn info(msg: &str) {
    log(Level::Info, msg);
}
pub fn warn(msg: &str) {
    log(Level::Warn, msg);
}
pub fn error(msg: &str) {
    log(Level::Error, msg);
}
pub fn trace(msg: &str) {
    log(Level::Trace, msg);
}

/// 启动时调用一次。重复调用安全（后一次生效）。
pub fn init() {
    let path = log_path();
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => {
            *LOG.lock().unwrap() = Some(f);
            info("=== tyl-app 启动 ===");
            info(&format!("日志: {}", path.display()));
            info(&format!("版本: {}", env!("CARGO_PKG_VERSION")));
        }
        Err(e) => eprintln!("[tyl] 日志初始化失败 {path:?}: {e}"),
    }
}

/// 日志文件路径：exe 目录优先（用户从 Downloads 跑，可写），否则临时目录。
pub fn log_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("tyl.log")))
        .filter(|p| OpenOptions::new().append(true).create(true).open(p).is_ok())
        .unwrap_or_else(|| std::env::temp_dir().join("tyl.log"))
}

pub fn log_str(msg: &str) {
    log(Level::Debug, msg);
}

fn log(level: Level, msg: &str) {
    if !allows(LEVEL.load(Ordering::Relaxed), level) {
        return;
    }
    let mut guard = LOG.lock().unwrap();
    if let Some(f) = guard.as_mut() {
        // One record per line; no escape/control sequences from remote errors.
        let msg: String = msg
            .chars()
            .take(2048)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let _ = writeln!(f, "[{}] [{level:?}] {msg}", timestamp());
    }
}

/// UTC time, useful for comparing intervals across the native/frontend logs.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}.{:03}", now.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn severity_filter_and_off() {
        for configured in [
            Level::Off,
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            for message in [
                Level::Error,
                Level::Warn,
                Level::Info,
                Level::Debug,
                Level::Trace,
            ] {
                assert_eq!(
                    allows(configured as u8, message),
                    message as u8 <= configured as u8
                );
            }
            assert!(!allows(configured as u8, Level::Off));
        }
    }
    #[test]
    fn level_json_round_trip() {
        for name in ["off", "error", "warn", "info", "debug", "trace"] {
            let value = format!("\"{name}\"");
            let level: Level = serde_json::from_str(&value).unwrap();
            assert_eq!(serde_json::to_string(&level).unwrap(), value);
        }
    }
}
