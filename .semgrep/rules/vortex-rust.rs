// Fichier de test des règles vortex-rust.yml (semgrep --test), jamais compilé.

fn open_with_system(path: &Path) {
    // ruleid: vortex-shell-command
    let mut c = std::process::Command::new("cmd");
    // ruleid: vortex-shell-command
    let p = Command::new("powershell");
    // ruleid: vortex-shell-command
    let s = Command::new("/bin/sh");
    // ruleid: vortex-shell-command
    let z = std::process::Command::new("zsh");
    // ruleid: vortex-shell-command
    let w = Command::new("C:\\Windows\\System32\\cmd.exe");
    // ok: vortex-shell-command
    let x = std::process::Command::new("xdg-open");
}

fn risky_to_open(name: &str) -> bool {
    // ruleid: vortex-extension-denylist
    name.ends_with(".exe")
}

fn safe_types() -> [&'static str; 2] {
    // ok: vortex-extension-denylist
    [".pdf", ".png"]
}

pub fn send(method: &str, url: &str, headers: &[(&str, String)]) {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        // ruleid: vortex-auth-redirect
        let response = send_once(&method, &current, headers, body, max_bytes)?;
        if let Some(location) = response.header("location") {
            current = resolve(&current, location);
            continue;
        }
    }
}

fn notify(summary: &str, body: &str) {
    // ruleid: vortex-notify-send-args
    let shown = std::process::Command::new("notify-send").arg(summary).arg(body).status();
}

fn notify_ok(summary: &str, body: &str) {
    // ok: vortex-notify-send-args
    let shown = std::process::Command::new("notify-send").args(notify_send_args(summary, body, "i", "c")).status();
}
