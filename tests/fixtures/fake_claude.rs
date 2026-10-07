// A stand-in for the claude CLI in integration tests: herdr detects it by
// name, it reports itself idle, and answers each line it reads.
use std::io::BufRead;
use std::process::Command;

fn report(state: &str) {
    let pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let _ = Command::new("herdr")
        .args(["pane", "report-agent", &pane, "--source", "fake", "--agent", "claude", "--state", state])
        .output();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("fake claude {}", args.join(" "));
    report("idle");
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        report("working");
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("reply to: {line}");
        report("idle");
    }
}
