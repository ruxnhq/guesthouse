//! The serial ctl channel in the Windows guest's report (windows/host/guest/report.ps1): its
//! validator against the host's grammar, its answer lines against the host's reading of them, and
//! its lock on the port. The block between the report's `ctl channel` markers is run on its own, as
//! the rest of the report changes the machine it runs on.

use crate::{Said, Scratch, quote, run, shell};
use guesthouse_remote_view_core::ctl;
use guesthouse_remote_view_core::ctl_wire::samples::{REFUSED, TAKEN};
use guesthouse_remote_view_core::ctl_wire::{
    Answer, Progress, Reassembly, answer_lines, base64_encode, parse_line, read_request,
};
use guesthouse_remote_view_core::share::PERMISSIONS;
use std::fs;
use std::path::{Path, PathBuf};

const BEGIN: &str = "# ctl channel: begin";
const END: &str = "# ctl channel: end";
const FP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn report() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../windows/host/guest/report.ps1")
}

/// The report's channel, in a file of its own, with what each test runs after it.
struct Channel {
    shell: &'static str,
    file: PathBuf,
    scratch: Scratch,
}

impl Channel {
    fn new(shell: &'static str) -> Self {
        let text = fs::read_to_string(report()).unwrap();
        let start = text.find(BEGIN).expect("the report marks where its channel begins");
        let end = text.find(END).expect("and where it ends");
        let scratch = Scratch::new();
        let file = scratch.path("ctl.ps1");
        fs::write(&file, &text[start..end + END.len()]).unwrap();
        Self { shell, file, scratch }
    }

    fn run(&self, then: &str) -> Said {
        let said =
            run(self.shell, &format!("$ErrorActionPreference = 'Stop'\n. {}\n{then}", quote(&self.file)));
        assert_eq!(said.code, Some(0), "{said}");
        said
    }
}

/// A PowerShell expression that is `text`, whatever it holds.
fn ps_text(text: &str) -> String {
    format!(
        "[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))",
        base64_encode(text.as_bytes())
    )
}

/// What the host's reading of a request line makes of it, as the guest's says it.
fn host_reads(line: &str) -> String {
    match read_request(line) {
        None => "none".into(),
        Some(Ok((id, words))) => format!("run {id} {}", words.join(" ")),
        Some(Err(id)) => format!("refuse {id}"),
    }
}

fn request_lines() -> Vec<String> {
    let mut words: Vec<String> = TAKEN.iter().chain(REFUSED).map(|line| (*line).to_string()).collect();
    // Every command this host makes, so a slip in any branch of the guest's grammar shows.
    let mut made = vec![
        ctl::status(),
        ctl::pair_open(),
        ctl::pair_close(),
        ctl::pair_accept(FP).unwrap(),
        ctl::pair_decline(FP).unwrap(),
        ctl::updates_off(),
        ctl::device_revoke(FP).unwrap(),
        ctl::device_drop(),
    ];
    for on in [true, false] {
        made.push(ctl::share(on));
        made.push(ctl::host_allow("clipboard", on).unwrap());
        made.push(ctl::host_allow("files", on).unwrap());
        for what in PERMISSIONS {
            made.push(ctl::device_allow(FP, what, on).unwrap());
        }
    }
    words.extend(made.iter().map(|made| made.join(" ")));
    let mut lines: Vec<String> = words.iter().map(|words| format!("guesthouse: ctl ask 5 {words}")).collect();
    lines.extend(
        [
            "guesthouse: ctl ask 0 status",
            "guesthouse: ctl ask 007 status",
            "guesthouse: ctl ask 4294967295 status",
            "guesthouse: ctl ask 4294967296 status",
            "guesthouse: ctl ask 99999999999 status",
            "guesthouse: ctl ask -1 status",
            "guesthouse: ctl ask 1",
            "guesthouse: ctl ask  status",
            "guesthouse: ctl ask 1 status\r",
            "guesthouse: ctl ask \u{661} status",
            "guesthouse: CTL ask 1 status",
            "guesthouse: ctl answer 1 0 1/1 -",
            "guesthouse: shutdown",
            "",
        ]
        .map(str::to_string),
    );
    lines
}

#[test]
fn the_guest_runs_and_refuses_exactly_what_the_host_would() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let lines = request_lines();
    let each: Vec<String> = lines.iter().map(|line| ps_text(line)).collect();
    let said = channel.run(&format!(
        "foreach ($line in @({})) {{\n\
           $request = Read-CtlRequest -Line $line\n\
           if (-not $request) {{ 'none' }}\n\
           elseif ($request.Valid) {{ \"run $($request.Id) $($request.Words -join ' ')\" }}\n\
           else {{ \"refuse $($request.Id)\" }}\n\
         }}",
        each.join(", ")
    ));
    let guest: Vec<&str> = said.stdout.lines().map(str::trim_end).collect();
    assert_eq!(guest.len(), lines.len(), "{said}");
    for (line, guest) in lines.iter().zip(guest) {
        assert_eq!(guest, host_reads(line), "{line:?}");
    }
    assert_eq!(host_reads("guesthouse: ctl ask 5 sharing on"), "run 5 sharing on");
    assert_eq!(host_reads("guesthouse: ctl ask 5 sharing off"), "refuse 5");
    assert_eq!(host_reads("guesthouse: ctl ask 5 drop"), "refuse 5");
    // The samples are both kinds, so neither side agrees by taking or refusing everything.
    assert!(lines.iter().any(|line| host_reads(line).starts_with("run ")));
    assert!(lines.iter().any(|line| host_reads(line).starts_with("refuse ")));
}

fn host_reassembles(id: u32, lines: &[&str]) -> Progress {
    let mut whole = Reassembly::new(id);
    let mut last = Progress::Ignored;
    for line in lines {
        last = whole.feed(&parse_line(line).unwrap_or_else(|| panic!("{line}")));
    }
    last
}

#[test]
fn a_long_answer_in_the_guests_lines_is_the_one_the_host_reads() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let devices: Vec<String> = (0..40)
        .map(|at| format!("{{\"name\":\"Zo\u{eb}'s \u{1f4f1} {at}\",\"fingerprint\":\"{FP}\"}}"))
        .collect();
    let status = format!("{{\"pairing\":true,\"code\":\"123456\",\"devices\":[{}]}}", devices.join(","));
    let said = channel.run(&format!(
        "Get-CtlAnswerLines -Id 17 -Code 0 -Bytes ([System.Text.Encoding]::UTF8.GetBytes({}))",
        ps_text(&status)
    ));
    let lines = said.lines();
    assert_eq!(lines, answer_lines(17, 0, status.as_bytes()).unwrap(), "{said}");
    assert!(lines.len() > 3);
    assert_eq!(host_reassembles(17, &lines), Progress::Done(Answer { code: 0, text: status }));
}

#[test]
fn an_empty_answer_and_one_too_long_read_as_the_host_expects() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let said = channel.run(
        "Get-CtlAnswerLines -Id 2 -Code -1 -Bytes ([byte[]]@())\n\
         Get-CtlAnswerLines -Id 3 -Code 0 -Bytes (New-Object byte[] 65537)",
    );
    assert_eq!(
        said.lines(),
        ["guesthouse: ctl answer 2 -1 1/1 -", "guesthouse: ctl failed 3 toolong"],
        "{said}"
    );
}

#[test]
fn a_request_is_answered_on_the_port_and_nothing_else_is() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let missing = quote(&channel.scratch.path("no-remoteman.exe"));
    let said = channel.run(&format!(
        "$port = New-Object System.IO.StringWriter\n\
         $port.NewLine = \"`n\"\n\
         $lock = New-Object System.Object\n\
         'card: ' + (Receive-CtlLine -Line 'guesthouse: card: ok' -Port $port -Lock $lock -Program {missing})\n\
         'shutdown: ' + (Receive-CtlLine -Line 'guesthouse: shutdown' -Port $port -Lock $lock -Program {missing})\n\
         'sharing: ' + (Receive-CtlLine -Line 'guesthouse: ctl ask 3 sharing off' -Port $port -Lock $lock -Program {missing})\n\
         'quote: ' + (Receive-CtlLine -Line \"guesthouse: ctl ask 4 pair 'on'\" -Port $port -Lock $lock -Program {missing})\n\
         'status: ' + (Receive-CtlLine -Line 'guesthouse: ctl ask 5 status' -Port $port -Lock $lock -Program {missing})\n\
         $port.ToString()"
    ));
    assert_eq!(
        said.lines(),
        [
            "card: False",
            "shutdown: False",
            "sharing: True",
            "quote: True",
            "status: True",
            "guesthouse: ctl failed 3 refused",
            "guesthouse: ctl failed 4 refused",
            "guesthouse: ctl failed 5 missing",
        ],
        "{said}"
    );
}

#[test]
fn a_program_is_run_with_its_words_as_arguments_and_nothing_a_shell_would_read() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let said = channel.run(
        "$program = (Get-Command -Name 'pwsh', 'powershell' -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source\n\
         foreach ($arguments in @(@('-NoProfile', '-Command', 'a;b'), @('-NoProfile', '-Command', 'a b'), @('x\"y'), @('$(calc)'))) {\n\
           'refused: ' + (Invoke-Ctl -Program $program -Arguments $arguments -Milliseconds 5000).Failure\n\
         }\n\
         $ran = Invoke-Ctl -Program $program -Arguments @('-NoProfile', '-NonInteractive', '-Command', 'exit') -Milliseconds 60000\n\
         'ran: ' + $ran.Failure + ' ' + $ran.Code",
    );
    assert_eq!(
        said.lines(),
        ["refused: refused", "refused: refused", "refused: refused", "refused: refused", "ran:  0"],
        "{said}"
    );
}

/// Windows' own programs stand in for remoteman's host: what one prints comes back as it said it,
/// and one that runs too long is stopped.
#[cfg(windows)]
#[test]
fn what_the_program_said_comes_back_and_one_too_slow_is_stopped() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let said = channel.run(
        "$ran = Invoke-Ctl -Program $env:ComSpec -Arguments @('/c', 'echo', 'pair', 'on') -Milliseconds 10000\n\
         'echo: ' + $ran.Code + ' ' + [System.Text.Encoding]::UTF8.GetString($ran.Bytes).Trim()\n\
         $failed = Invoke-Ctl -Program $env:ComSpec -Arguments @('/c', 'exit', '3') -Milliseconds 10000\n\
         'exit: ' + $failed.Code\n\
         $started = Get-Date\n\
         $slow = Invoke-Ctl -Program (Join-Path $env:SystemRoot 'System32\\PING.EXE') -Arguments @('-n', '30', '127.0.0.1') -Milliseconds 500\n\
         'slow: ' + $slow.Failure + ' ' + (((Get-Date) - $started).TotalSeconds -lt 5)",
    );
    assert_eq!(said.lines(), ["echo: 0 pair on", "exit: 3", "slow: timeout True"], "{said}");
}

/// Two threads write whole answers through the lock to a port that writes a character at a time,
/// as the report and its listener do, and no line is cut into by another. Each has the channel's
/// functions as the report hands them to its listener's thread.
#[test]
fn lines_written_under_the_lock_from_two_threads_stay_whole() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    let said = channel.run(
        "Add-Type -TypeDefinition @'\n\
         public class SlowPort {\n\
           public System.Text.StringBuilder Written = new System.Text.StringBuilder();\n\
           public void WriteLine(string line) {\n\
             foreach (char c in line + \"\\n\") { lock (Written) { Written.Append(c); } System.Threading.Thread.Sleep(0); }\n\
           }\n\
         }\n\
         '@\n\
         $port = New-Object SlowPort\n\
         $lock = New-Object System.Object\n\
         $writers = foreach ($name in 'one', 'two') {\n\
           $writer = [powershell]::Create()\n\
           [void]$writer.AddScript({\n\
             param($port, $lock, $code, $name)\n\
             . ([scriptblock]::Create($code))\n\
             for ($n = 0; $n -lt 40; $n += 1) {\n\
               Write-CtlLines -Port $port -Lock $lock -Lines @(\"$name $n a long line of the answer, $('x' * 200)\", \"$name $n its second line\")\n\
             }\n\
           }).AddArgument($port).AddArgument($lock).AddArgument($ctlCode).AddArgument($name)\n\
           [pscustomobject]@{ Shell = $writer; Running = $writer.BeginInvoke() }\n\
         }\n\
         foreach ($writer in $writers) { $writer.Shell.EndInvoke($writer.Running); $writer.Shell.Dispose() }\n\
         $port.Written.ToString()",
    );
    let lines = said.lines();
    assert_eq!(lines.len(), 160, "{said}");
    let long = format!(" a long line of the answer, {}", "x".repeat(200));
    for (at, pair) in lines.chunks(2).enumerate() {
        let (name, rest) = pair[0].split_once(' ').unwrap_or_else(|| panic!("line {at}: {:?}", pair[0]));
        let number = rest.strip_suffix(&long).unwrap_or_else(|| panic!("line {at}: {:?}", pair[0]));
        assert!(["one", "two"].contains(&name), "{:?}", pair[0]);
        assert_eq!(pair[1], format!("{name} {number} its second line"), "an answer's lines stay together");
    }
}

/// What the host reads from the lines `Receive-CtlLine` wrote for request 7, with remoteman's host a
/// stand-in that says `status` as `status` and notes every other command it is given.
fn answer_with_waiting(channel: &Channel, line: &str, status: &str) -> (String, Vec<String>) {
    let said = channel.run(&format!(
        "$script:ran = New-Object System.Collections.Generic.List[string]\n\
         function Invoke-Ctl {{\n\
           param([string]$Program, [string[]]$Arguments, [int]$Milliseconds)\n\
           $rest = @($Arguments | Select-Object -Skip 3)\n\
           if ($rest -ceq 'status') {{ $text = {status} }} else {{ $script:ran.Add($rest -join ' '); $text = 'ok' }}\n\
           [pscustomobject]@{{ Failure = $null; Code = 0; Bytes = [System.Text.Encoding]::UTF8.GetBytes($text) }}\n\
         }}\n\
         $port = New-Object System.IO.StringWriter\n\
         $port.NewLine = \"`n\"\n\
         $null = Receive-CtlLine -Line ({line}) -Port $port -Lock (New-Object System.Object) -Program 'x'\n\
         foreach ($one in $script:ran) {{ 'ran: ' + $one }}\n\
         $port.ToString()",
        line = ps_text(line),
    ));
    let ran = said.lines().iter().filter_map(|line| line.strip_prefix("ran: ")).map(str::to_owned).collect();
    let answer: Vec<&str> =
        said.lines().into_iter().filter(|line| line.starts_with("guesthouse: ")).collect();
    let text = match host_reassembles(7, &answer) {
        Progress::Done(Answer { text, .. }) => text,
        other => format!("no answer, {other:?}: {said}"),
    };
    (text, ran)
}

#[test]
fn an_answer_to_pairing_goes_only_to_the_device_named() {
    let Some(shell) = shell() else { return };
    let channel = Channel::new(shell);
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    let waiting = |fingerprint: &str| {
        ps_text(&format!(
            "warning first\r\n{{\"pairing\":false,\"pending\":{{\"name\":\"Tab\",\"fingerprint\":\"{fingerprint}\",\"code\":\"111 222\",\"seconds\":40}}}}\r\n"
        ))
    };
    for answer in ["accept", "decline"] {
        let line = format!("guesthouse: ctl ask 7 pair {answer} {FP}");
        let (text, ran) = answer_with_waiting(&channel, &line, &waiting(&FP.to_ascii_uppercase()));
        assert_eq!((text.as_str(), ran), ("ok", vec![format!("pair {answer}")]));
        for status in [waiting(OTHER), ps_text("{\"pairing\":true,\"pending\":null}"), ps_text("not json")] {
            let (text, ran) = answer_with_waiting(&channel, &line, &status);
            assert_eq!(text, ctl::ANOTHER_DEVICE_WAITS, "{answer} with {status}");
            assert!(ran.is_empty(), "{answer} with {status}: {ran:?}");
        }
    }
}
