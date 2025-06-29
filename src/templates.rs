use teloxide::{ types::ParseMode };
use crate::models::Alert;

pub fn escape(text: &str) -> String {
    let reserved_chars = [
        '_',
        '*',
        '[',
        ']',
        '(',
        ')',
        '~',
        '`',
        '#',
        '+',
        '-',
        '=',
        '|',
        '{',
        '}',
        '.',
        '!',
    ];
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if reserved_chars.contains(&c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

pub fn format_alert(alert: &Alert) -> (String, ParseMode) {
    (
        format!(
            "🚨 *[{}] Alert from {}* 🚨\n\n*Host:* `{}`\n*IP:* `{}`\n\n*Log:*\n```\n{}\n```",
            escape(&alert.event_type),
            escape(&alert.slave_alias),
            escape(&alert.hostname),
            escape(&alert.server_ip),
            &alert.log_line
        ),
        ParseMode::MarkdownV2,
    )
}

pub fn format_health_report(report_lines: Vec<String>) -> (String, ParseMode) {
    let mut report = String::from("*— Health Report —*\n");
    for line in report_lines {
        report.push_str(&escape(&line));
        report.push('\n');
    }
    (report, ParseMode::MarkdownV2)
}

pub fn format_who_output(alias: &str, who_output: &str) -> (String, ParseMode) {
    (format!("*— Who on {} —*\n```text\n{}```", escape(alias), who_output), ParseMode::MarkdownV2)
}

pub fn format_node_summary(summary_lines: Vec<String>) -> (String, ParseMode) {
    (format!("*— Node Summary —*\n\n{}", summary_lines.join("\n\n")), ParseMode::MarkdownV2)
}
