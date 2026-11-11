use rivet_copilot::{Event, ProbeOptions};
use std::{
    io::{self, Write},
    time::Duration,
};

const HELP: &str = "Rivet — Native agent workspace prototype\n\nUsage:\n  rivet ui [--smoke] (macOS only)\n  rivet bench-ui\n  rivet doctor\n  rivet auth\n  rivet chat PROMPT [--resume SESSION] [--cancel-after-ms N] [--timeout-secs N]\n  rivet resume SESSION\n\nRun from the intended workspace. All tool permission requests are denied.\nPrompts may incur Copilot usage. No credentials are printed or stored by Rivet.";

fn parse(args: &[String]) -> Result<Option<ProbeOptions>, String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(None);
    };
    if matches!(command, "--help" | "-h") {
        return Ok(None);
    }
    let mut options = ProbeOptions {
        workspace: std::env::current_dir().map_err(|e| e.to_string())?,
        prompt: None,
        resume: None,
        cancel_after: None,
        timeout: Duration::from_secs(120),
    };
    match command {
        "auth" if args.len() == 1 => {}
        "resume" if args.len() == 2 => options.resume = Some(args[1].clone()),
        "chat" if args.len() >= 2 => {
            options.prompt = Some(args[1].clone());
            let mut index = 2;
            while index < args.len() {
                let flag = &args[index];
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--resume" => options.resume = Some(value.clone()),
                    "--cancel-after-ms" => {
                        options.cancel_after = Some(Duration::from_millis(number(value)?))
                    }
                    "--timeout-secs" => options.timeout = Duration::from_secs(number(value)?),
                    _ => return Err(format!("unknown option: {flag}")),
                }
                index += 2;
            }
        }
        _ => return Err("invalid command; use --help".into()),
    }
    Ok(Some(options))
}
fn number(value: &str) -> Result<u64, String> {
    let n = value
        .parse::<u64>()
        .map_err(|_| "expected a positive integer".to_string())?;
    if n == 0 || n > 86_400 {
        return Err("duration must be between 1 and 86400".into());
    }
    Ok(n)
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(target_os = "macos")]
    if args.is_empty() {
        return rivet_macos::run(false);
    }
    if args == ["bench-ui"] {
        print!("{}", rivet_prototype::benchmark());
        return Ok(());
    }
    if args == ["ui"] || args == ["ui", "--smoke"] {
        #[cfg(target_os = "macos")]
        return rivet_macos::run(args.len() == 2);
        #[cfg(not(target_os = "macos"))]
        return Err("The Stage 2 native UI is macOS-only. Windows UI follows later; CLI probes and bench-ui work here.".into());
    }
    if args == ["doctor"] {
        println!(
            "Rivet Stage 1; host={}-{}; UI targets=macOS, Windows",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        match std::process::Command::new("git").arg("--version").output() {
            Ok(output) if output.status.success() => {
                print!("{}", String::from_utf8_lossy(&output.stdout))
            }
            _ => println!("Git unavailable; install Git before repository operations."),
        }
        println!(
            "Copilot SDK: 1.0.32-preview.1; bundled runtime; permission policy: deny all\nAuthentication is checked separately with `rivet auth`."
        );
        return Ok(());
    }
    let Some(options) = parse(&args)? else {
        println!("{HELP}");
        return Ok(());
    };
    rivet_copilot::probe(options, |event| match event {
        Event::Text(text) => {
            print!("{text}");
            let _ = io::stdout().flush();
        }
        Event::Authenticated(ok) => eprintln!("authenticated={ok}"),
        Event::Session(id) => eprintln!("session={id}"),
        Event::Timing { operation, micros } => {
            eprintln!("{operation}={micros}us (includes runtime/network work)")
        }
        Event::Activity(kind) => eprintln!("activity={kind}"),
        Event::Completed => eprintln!("\ncompleted"),
        Event::Cancelled => eprintln!("\ncancel acknowledged by SDK"),
    })
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Rivet: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn rejects_incomplete_or_unknown_commands() {
        for a in [
            &["chat"][..],
            &["resume"],
            &["auth", "extra"],
            &["chat", "hi", "--unknown", "1"],
            &["chat", "hi", "--timeout-secs", "0"],
        ] {
            assert!(parse(&args(a)).is_err(), "{a:?}");
        }
    }
    #[test]
    fn parses_resume_and_cancellation() {
        let o = parse(&args(&[
            "chat",
            "hello",
            "--resume",
            "abc",
            "--cancel-after-ms",
            "100",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(o.resume.as_deref(), Some("abc"));
        assert_eq!(o.cancel_after, Some(Duration::from_millis(100)));
    }
}
