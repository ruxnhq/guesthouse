//! Lean Windows' step in a Windows guest's answer file on the Linux host. Setup's specialize pass is
//! the last moment before any account exists, so what it takes out is never installed for the new
//! account. Whether a machine is lean, and the script and list the step runs, stay with the host.

/// The answer file's own commands come first.
const SPECIALIZE_END: &str = "      </RunSynchronous>";
pub const SPECIALIZE_COMMAND: &str = "        <RunSynchronousCommand wcm:action=\"add\"><Order>5</Order><Path>cmd /c for %i in (D E F G H) do if exist %i:\\lean.ps1 powershell -NoProfile -ExecutionPolicy Bypass -File %i:\\lean.ps1 -Data %i:\\lean.json</Path></RunSynchronousCommand>\n";
/// What the host says when an answer file has no one place for the step.
pub const NO_PLACE: &str = "The answer file has no place for lean Windows' step, so nothing was installed.";

/// The filled answer file with one command more in the specialize pass, which runs the script on the
/// disc. The template is never changed, so an answer file without it is the same as before.
pub fn with_specialize_step(answers: &str) -> Result<String, &'static str> {
    if answers.matches(SPECIALIZE_END).count() != 1 {
        return Err(NO_PLACE);
    }
    Ok(answers.replacen(SPECIALIZE_END, &format!("{SPECIALIZE_COMMAND}{SPECIALIZE_END}"), 1))
}
