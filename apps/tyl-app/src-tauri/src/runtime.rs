//! 全局共享 tokio runtime（翻译等异步任务）。
//! Tauri 自带的 runtime 不暴露 handle，自建一个多线程实例最省心。

use std::sync::OnceLock;

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// 取共享 runtime 的 handle（惰性初始化，进程生命周期一个实例）。
pub fn handle() -> tokio::runtime::Handle {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build tokio runtime")
    })
    .handle()
    .clone()
}
