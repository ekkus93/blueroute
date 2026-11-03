//! Typed troubleshooting guidance for CLI users and JSON automation.
//! Advice is chosen from the exit category, not untrusted daemon error text.
const GUIDE: &str =
    "https://github.com/ekkus93/blueroute/blob/master/docs/TROUBLESHOOTING.md";
const ADMIN_GUIDE: &str =
    "https://github.com/ekkus93/blueroute/blob/master/docs/ADMINISTRATOR.md";

pub(crate) fn format_failure(code: i32, message: &str, json: bool) -> String {
    let (kind, next_step, help) = match code {
        2 => (
            "usage",
            "Run `blueroute --help` to see supported commands and options.",
            ADMIN_GUIDE.to_owned(),
        ),
        3 => (
            "daemon_unavailable",
            "Check `systemctl status blueroute.service` and the daemon journal.",
            format!("{GUIDE}#the-daemon-is-unavailable-or-reports-unsupported-capability"),
        ),
        4 => (
            "authorization",
            "The operation needs PolicyKit authorization; do not bypass it with sudo.",
            format!("{GUIDE}#permission-denied"),
        ),
        5 => (
            "unsupported",
            "Check `blueroute capability show`; authenticated network join is not yet enabled.",
            format!("{GUIDE}#the-daemon-is-unavailable-or-reports-unsupported-capability"),
        ),
        _ => (
            "operational",
            "Check `blueroute diagnostics show` and the daemon journal.",
            format!("{GUIDE}#common-failures"),
        ),
    };
    if json {
        // Errors use stderr; successful --json responses still use stdout.
        serde_json::json!({
            "ok": false,
            "error": { "kind": kind, "code": code, "message": message, "help": help }
        })
        .to_string()
    } else {
        format!("blueroute: {message}\n{next_step}\nGuide: {help}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_have_stable_machine_readable_categories_and_links() {
        for (code, category, path) in [
            (1, "operational", "#common-failures"),
            (2, "usage", "ADMINISTRATOR.md"),
            (3, "daemon_unavailable", "#the-daemon-is-unavailable"),
            (4, "authorization", "#permission-denied"),
            (5, "unsupported", "#the-daemon-is-unavailable"),
        ] {
            let rendered = format_failure(code, "test failure", true);
            let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(value["ok"], false);
            assert_eq!(value["error"]["code"], code);
            assert_eq!(value["error"]["kind"], category);
            assert_eq!(value["error"]["message"], "test failure");
            assert!(
                value["error"]["help"].as_str().unwrap().contains(path),
                "{rendered}"
            );
        }
    }

    #[test]
    fn messages_are_escaped_as_data_not_json_syntax() {
        let msg = "failure: \"bad\"\\nnot a new field";
        let rendered = format_failure(3, msg, true);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["error"]["message"], msg);
        assert_eq!(parsed["error"].as_object().unwrap().len(), 4);
    }

    #[test]
    fn human_guidance_is_actionable_and_does_not_reclassify_by_message() {
        let denied = format_failure(4, "contains NotSupported", false);
        assert!(denied.contains("PolicyKit"));
        assert!(denied.contains("#permission-denied"));
        let unsupported = format_failure(5, "contains AccessDenied", false);
        assert!(unsupported.contains("authenticated network join is not yet enabled"));
        assert!(!unsupported.contains("#permission-denied"));
        assert!(format_failure(2, "invalid option", false).contains("blueroute --help"));
    }
}
