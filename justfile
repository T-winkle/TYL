# TYL 常用命令。用法: just <target>（无 just 时按注释里的裸命令执行）

# 默认：列出所有目标
default:
    @just --list

# Linux 本机：core 测试 + 全 workspace 格式检查
test:
    cargo test -p tyl-core
    cargo fmt --all --check

# Windows 代码静态检查（不产物，最快）
check-win:
    cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings

# Windows 交叉构建 exe（clang-cl + lld-link，产出与 CI windows runner 同源 MSVC 二进制）
# 静态链接 CRT（.cargo/config.toml 已配置）→ 拷到任意 Win10/11 即可运行，无需 VC++ 运行库
# 依赖：sudo apt install clang lld llvm && cargo install cargo-xwin
build-win:
    cargo xwin build --release --target x86_64-pc-windows-msvc -p tyl-cli

# Tauri 应用（先构建前端再交叉编译；产物 ~9.5MB 单文件，前端已内嵌）
build-app-win:
    npm --prefix apps/tyl-app/ui run build
    cargo xwin build --release --target x86_64-pc-windows-msvc -p tyl-app

# 清理
clean:
    cargo clean
