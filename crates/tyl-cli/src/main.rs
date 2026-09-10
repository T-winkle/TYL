//! tyl-cli — M1 verification tool for the capture engine.
//!
//! Subcommands:
//!   anchor          Print cursor + foreground process (no side effects)
//!   capture         Run the full pipeline on the current selection
//!   monitors        Print monitor topology + DPI scales
//!
//! Exit codes: 0 = success, 1 = capture failure, 2 = usage error.

use std::time::Duration;

use serde_json::json;

use tyl_core::capture::CaptureOutcome;
use tyl_core::config::FallbackPolicy;
use tyl_core::ScreenLocator;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("anchor") => cmd_anchor(),
        Some("capture") => cmd_capture(&args[1..]),
        Some("monitors") => cmd_monitors(),
        Some("selfcheck") => cmd_selfcheck(),
        _ => {
            eprintln!(
                "tyl-cli {version}\n\
                 \n\
                 USAGE:\n\
                 \x20   tyl-cli <COMMAND> [OPTIONS]\n\
                 \n\
                 COMMANDS:\n\
                 \x20   anchor                   Print cursor + foreground process\n\
                 \x20   capture [--json] [--timeout MS] [--delay MS]\n\
                 \x20           [--no-fallback] [--clipboard-only] [--no-restore]\n\
                 \x20                            Capture the current selection\n\
                 \x20   monitors                 Print monitors + DPI\n\
                 \x20   selfcheck                No-op: survives a simulated Ctrl+C test\n\
                 \n\
                 capture exits 1 on failure, 2 on usage error.",
                version = env!("CARGO_PKG_VERSION")
            );
            2
        }
    };
    std::process::exit(code);
}

fn cmd_anchor() -> i32 {
    let anchor = tyl_platform::backend::capture_anchor();
    println!(
        "{}",
        json!({
            "cursor": anchor.cursor,
            "target_exe": anchor.target_exe,
        })
    );
    0
}

/// Verifies the DPI-awareness fix ran (monitors report real resolution, not
/// 96-DPI virtualized values) and prints environment diagnostics.
fn cmd_selfcheck() -> i32 {
    tyl_platform::backend::enable_dpi_awareness();
    let monitors = <tyl_platform::backend::WinLocator as Default>::default().monitors();
    let suspicious = monitors
        .iter()
        .any(|m| (m.rect.right - m.rect.left) % 96 != 0 && m.scale == 1.0 && m.rect.right > 0);
    println!(
        "{}",
        json!({
            "monitors": monitors,
            "note": if suspicious {
                "at least one monitor reports unusual geometry — compare with Settings > Display"
            } else {
                "ok"
            },
        })
    );
    0
}

fn cmd_monitors() -> i32 {
    tyl_platform::backend::enable_dpi_awareness();
    let monitors = <tyl_platform::backend::WinLocator as Default>::default().monitors();
    println!("{}", json!({ "monitors": monitors }));
    0
}

fn cmd_capture(args: &[String]) -> i32 {
    tyl_platform::backend::enable_dpi_awareness();
    let mut json_out = false;
    let mut timeout = 800u64;
    let mut delay = 350u64; // give the human time to release the hotkey
    let mut policy = FallbackPolicy::Auto;
    let mut restore = true;
    let mut verbose = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json_out = true,
            "--verbose" | "-v" => verbose = true,
            "--timeout" => match it.next().and_then(|v| v.parse().ok()) {
                Some(ms) => timeout = ms,
                None => return usage_err("--timeout needs a millisecond value"),
            },
            "--delay" => match it.next().and_then(|v| v.parse().ok()) {
                Some(ms) => delay = ms,
                None => return usage_err("--delay needs a millisecond value"),
            },
            "--no-fallback" => policy = FallbackPolicy::UiaOnly,
            "--clipboard-only" => policy = FallbackPolicy::ClipboardOnly,
            "--no-restore" => restore = false,
            other => return usage_err(&format!("unknown option: {other}")),
        }
    }

    if delay > 0 {
        // The capture targets whatever window is foreground *after* this
        // delay — so the runbook is: start the command, then focus the app
        // with the text selected while the countdown runs.
        eprintln!("# select text and focus the target app now…");
        let remaining = delay;
        let mut left = remaining;
        while left > 0 {
            eprint!(
                "\r# capturing in {::<4}s (focus the target app!)  ",
                left.div_ceil(1000)
            );
            let step = left.min(250);
            std::thread::sleep(Duration::from_millis(step));
            left -= step;
        }
        eprintln!("\r# capturing…                                        ");
    }

    let capture = tyl_platform::backend::WinCapture::with_policy_and_restore(policy, restore);
    let anchor = tyl_platform::backend::capture_anchor();
    // Target diagnostics on stderr so `--json` stdout stays parseable.
    eprintln!(
        "# target: {} cursor: {:?}",
        anchor.target_exe.as_deref().unwrap_or("?"),
        anchor.cursor
    );
    match capture.capture_detailed(&anchor, Duration::from_millis(timeout)) {
        CaptureOutcome::Primary(t) => report(&t, "primary(uia)", json_out, verbose),
        CaptureOutcome::Fallback(t) => report(&t, "fallback(clipboard)", json_out, verbose),
        CaptureOutcome::Failed(e) => {
            // Both streams: JSON parsers read stdout, humans read stderr.
            if json_out {
                println!("{}", json!({ "ok": false, "error": e.to_string() }));
            }
            eprintln!("capture failed: {e}");
            1
        }
    }
}

fn report(t: &tyl_core::CapturedText, channel: &str, json_out: bool, verbose: bool) -> i32 {
    if json_out {
        let mut v = json!({
            "ok": true,
            "channel": channel,
            "source": t.source,
            "text": t.text,
            "elapsed_ms": t.elapsed.as_millis() as u64,
        });
        if verbose {
            v["selection_rect"] = json!(t.selection_rect);
        }
        println!("{v}");
    } else {
        println!("channel: {channel}");
        println!("source:  {:?}", t.source);
        if verbose {
            println!("rect:    {:?}", t.selection_rect);
        }
        println!("elapsed: {:.1} ms", t.elapsed.as_secs_f64() * 1000.0);
        println!("--- text ---");
        println!("{}", t.text);
    }
    0
}

fn usage_err(msg: &str) -> i32 {
    eprintln!("error: {msg}\nrun `tyl-cli` with no arguments for usage");
    2
}
