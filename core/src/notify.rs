pub fn send_notification(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"display notification "{}" with title "{}""#,
            body.replace('"', "\\\""),
            title.replace('"', "\\\""),
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .arg("-i")
            .arg("dialog-information")
            .arg(title)
            .arg(body)
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "[System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms'); \
             [System.Windows.Forms.MessageBox]::Show('{}', '{}')",
            body.replace('\'', "\\'"),
            title.replace('\'', "\\'"),
        );
        let _ = std::process::Command::new("powershell.exe")
            .arg("-c")
            .arg(&script)
            .status();
    }
    // Suppress unused-variable warnings when none of the above cfg branches match.
    let _ = (title, body);
}
