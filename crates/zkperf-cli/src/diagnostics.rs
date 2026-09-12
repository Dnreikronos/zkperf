use std::io::{self, Write};

use crate::args::LogLevel;

#[derive(Debug)]
pub struct Diagnostic {
    pub exit_code: u8,
    category: &'static str,
    message: String,
}

impl Diagnostic {
    pub fn runtime(message: impl Into<String>, cancelled: bool) -> Self {
        Self {
            exit_code: if cancelled { 130 } else { 6 },
            category: if cancelled { "cancelled" } else { "runtime" },
            message: message.into(),
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self {
            exit_code: 3,
            category: "configuration",
            message: message.into(),
        }
    }

    pub fn unavailable(command: &str, issue: &str) -> Self {
        Self {
            exit_code: 5,
            category: "unavailable",
            message: format!("{command} is not implemented yet; tracked in {issue}"),
        }
    }

    pub fn emit(&self, stderr: &mut impl Write) -> io::Result<()> {
        writeln!(
            stderr,
            "zkperf: error[{}]: {}",
            self.category,
            single_line(&self.message)
        )
    }
}

impl From<io::Error> for Diagnostic {
    fn from(error: io::Error) -> Self {
        Self {
            exit_code: 4,
            category: "io",
            message: error.to_string(),
        }
    }
}

pub struct Logger<W> {
    level: LogLevel,
    writer: W,
}

impl<W: Write> Logger<W> {
    pub const fn new(level: LogLevel, writer: W) -> Self {
        Self { level, writer }
    }

    pub fn log(&mut self, level: LogLevel, message: &str) -> Result<(), Diagnostic> {
        if level != LogLevel::Off && self.level >= level {
            writeln!(self.writer, "zkperf: {level:?}: {}", single_line(message))
                .map_err(Diagnostic::from)?;
        }
        Ok(())
    }
}

pub fn write_output(stdout: &mut impl Write, output: &str) -> Result<(), Diagnostic> {
    writeln!(stdout, "{output}")
        .and_then(|()| stdout.flush())
        .map_err(Diagnostic::from)
}

fn single_line(message: &str) -> String {
    message.chars().flat_map(char::escape_debug).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_failures_have_io_status() {
        assert_eq!(
            write_output(&mut BrokenWriter, "result")
                .unwrap_err()
                .exit_code,
            4
        );
        assert_eq!(
            Logger::new(LogLevel::Info, BrokenWriter)
                .log(LogLevel::Info, "progress")
                .unwrap_err()
                .exit_code,
            4
        );
    }

    #[test]
    fn quiet_logging_does_not_hide_errors_and_messages_stay_on_one_line() {
        let mut stderr = Vec::new();
        Logger::new(LogLevel::Off, &mut stderr)
            .log(LogLevel::Error, "hidden")
            .unwrap();
        assert!(stderr.is_empty());
        Diagnostic::configuration("field: bad\ninjected message")
            .emit(&mut stderr)
            .unwrap();
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "zkperf: error[configuration]: field: bad\\ninjected message\n"
        );
    }
}
