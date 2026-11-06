use std::env;
use std::io::IsTerminal;

/// 终端配色。非 TTY 或 NO_COLOR 时全部退化为空串，管道里不会出现转义码。
pub struct Style {
    color: bool,
    orange: &'static str,
}

impl Style {
    pub fn detect() -> Self {
        let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
        let orange = if !color {
            ""
        } else if env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false)
        {
            // Claude 橙 #D97757
            "\x1b[38;2;217;119;87m"
        } else if env::var("TERM")
            .map(|v| v.contains("256color"))
            .unwrap_or(false)
        {
            "\x1b[38;5;173m"
        } else {
            "\x1b[33m"
        };
        Self { color, orange }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.color && !code.is_empty() {
            format!("{code}{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.wrap("\x1b[1m", text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.wrap("\x1b[2m", text)
    }
    pub fn cyan(&self, text: &str) -> String {
        self.wrap("\x1b[36m", text)
    }
    pub fn green(&self, text: &str) -> String {
        self.wrap("\x1b[32m", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.wrap("\x1b[31m", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.wrap("\x1b[33m", text)
    }
    pub fn tier(&self, text: &str) -> String {
        self.wrap(self.orange, text)
    }
    pub fn section(&self, text: &str) -> String {
        self.cyan(&format!("== {text} =="))
    }
    pub fn ok(&self, text: &str) -> String {
        format!("{} {text}", self.green("✓"))
    }
    pub fn warn(&self, text: &str) -> String {
        format!("{} {text}", self.yellow("!"))
    }
    pub fn fail(&self, text: &str) -> String {
        format!("{} {text}", self.red("✗"))
    }
}
