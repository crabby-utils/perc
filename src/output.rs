use console::style;
use serde::Serialize;

pub struct Output {
    json: bool,
}

impl Output {
    #[must_use]
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    pub fn success<T: Serialize>(&self, data: &T) {
        if self.json {
            println!("{}", serde_json::to_string(data).unwrap());
        } else {
            println!(
                "{} {}",
                style("✔").green(),
                serde_json::to_value(data)
                    .and_then(|v| serde_json::to_string_pretty(&v))
                    .unwrap()
            );
        }
    }

    pub fn step(&self, step: &str, message: &str) {
        if self.json {
            let obj = serde_json::json!({ "step": step, "message": message });
            eprintln!("{}", serde_json::to_string(&obj).unwrap());
        } else {
            eprintln!("{} {message}", style("→").cyan());
        }
    }

    pub fn log_line(&self, line: &str) {
        if self.json {
            let obj = serde_json::json!({ "line": line });
            println!("{}", serde_json::to_string(&obj).unwrap());
        } else {
            println!("{line}");
        }
    }

    pub fn logs(&self, text: &str) {
        if self.json {
            let obj = serde_json::json!({ "logs": text });
            println!("{}", serde_json::to_string(&obj).unwrap());
        } else {
            print!("{text}");
        }
    }

    pub fn error(&self, code: &str, message: &str) {
        if self.json {
            let err = serde_json::json!({
                "error": message,
                "code": code,
            });
            eprintln!("{}", serde_json::to_string(&err).unwrap());
        } else {
            eprintln!("{} {}", style("error:").red().bold(), message);
        }
    }
}
