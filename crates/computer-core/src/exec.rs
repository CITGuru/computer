//! One command, one answer.

/// What a command inside the box did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,

    /// Not inferable from `code`. `timeout(1)` reports 124, and so does a
    /// command that exited 124 on its own; the runner records which happened
    /// rather than leaving every caller to guess the same way.
    pub timed_out: bool,
}

impl ExecResult {
    pub fn ok(&self) -> bool {
        self.code == 0 && !self.timed_out
    }

    pub fn stdout_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_timeout_is_not_a_success_whatever_the_code_says() {
        let result = ExecResult {
            code: 0,
            timed_out: true,
            ..ExecResult::default()
        };
        assert!(!result.ok(), "killed mid-run is not a clean exit");
    }

    #[test]
    fn test_exit_124_alone_does_not_mean_timed_out() {
        let result = ExecResult {
            code: 124,
            ..ExecResult::default()
        };
        assert!(
            !result.timed_out,
            "the runner records this; the code cannot say it"
        );
    }

    #[test]
    fn test_output_survives_invalid_utf8() {
        let result = ExecResult {
            stdout: vec![0xff, 0xfe, b'o', b'k'],
            ..ExecResult::default()
        };
        assert!(result.stdout_utf8().ends_with("ok"));
    }
}
