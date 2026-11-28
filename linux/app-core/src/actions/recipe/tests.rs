use super::super::tests::{machine, plain_task_context, with_vm_running};
use super::windows::{Progress, read_progress};
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::guest_shell::ShellAnswer;
use crate::guest_shell::stand_in::ScriptedShell;
use crate::machine::{Kind, Machine};
use crate::recipe::{Recipe, Source};
use crate::tasks::read_task;
use crate::testing::Scratch;
use crate::text::base64_decode;
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::sync::Arc;

const HOSTILE_SCRIPT: &str = "'; Remove-Item C:\\ -Recurse -Force #\n$(x) `y` \"z\"\nstep 1 apt evil\n";

fn chosen(guest: &str, packages: &[&str], script: Option<&str>) -> Chosen {
    let list: Vec<String> = packages.iter().map(|name| format!("{name:?}")).collect();
    let mut text = format!(
        "name = \"Box\"\ndescription = \"A box.\"\nguest = \"{guest}\"\n[steps]\npackages = [{}]\n",
        list.join(", ")
    );
    if let Some(script) = script {
        text.push_str(&format!("script = {script:?}\n"));
    }
    Chosen { source: Source::Yours, file: "/x.toml".into(), recipe: Recipe::parse(&text).unwrap() }
}

fn windows_context(scratch: &Scratch) -> Context {
    let context = plain_task_context(scratch);
    scratch.write("program/guest/recipe-start.ps1", "START\n");
    scratch.write("program/guest/recipe-run.ps1", "RUNNER\n");
    with_vm_running(&context);
    context
}

fn windows_machine(context: &Context, chosen: &Chosen) -> Machine {
    let vm = Machine { kind: Kind::Windows, ..machine(context) };
    crate::agent::make_token(context, &vm).unwrap();
    crate::recipe::write_chosen(context, &vm, chosen).unwrap();
    vm
}

/// What the guest was asked, the start script's data decoded, and the whole of what was sent.
fn data_sent(agent: &ScriptedAgent) -> serde_json::Value {
    let requests = agent.requests.lock().unwrap();
    let start =
        requests.iter().map(|(_, _, script)| script).find(|script| script.starts_with("$data = '")).unwrap();
    let encoded = start.split('\'').nth(1).unwrap();
    serde_json::from_slice(&base64_decode(encoded).unwrap()).unwrap()
}

fn guest_script(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guest/windows").join(name);
    fs::read_to_string(path).unwrap()
}

#[test]
fn running_the_steps_again_ends_the_earlier_runs_own_process_tree_first() {
    let run = guest_script("recipe-run.ps1");
    let wrote = run.find("Set-Content -LiteralPath (Join-Path $folder 'run.pid') -Value $PID").unwrap();
    assert!(wrote < run.find("Say 'begin'").unwrap(), "written before any step starts");
    let start = guest_script("recipe-start.ps1");
    let read = start.find("Get-Content -LiteralPath \"$folder\\run.pid\"").unwrap();
    let only_the_runner = start.find("CommandLine -like '*-File C:\\gpupv\\recipe\\run.ps1*'").unwrap();
    let ended = start.find("taskkill.exe /PID $earlier /T /F").unwrap();
    let cleared = start.find("Remove-Item \"$folder\\*\"").unwrap();
    assert!(read < only_the_runner && only_the_runner < ended && ended < cleared, "{start}");
    assert!(!start.contains("Stop-Process -Name") && !start.contains("/IM "), "never by name");
}

#[test]
fn the_status_file_is_read_line_by_line() {
    let progress = read_progress("begin\nwaiting winget\nstart 1\nok 1\nstart 2\n");
    assert_eq!(progress, Progress { running: Some(2), done: 1, ..Progress::default() });
    let failed = read_progress("begin\nstart 1\nfail 1 winget stopped with 5. No.\nend\n");
    assert_eq!(failed.failed, Some((1, "winget stopped with 5. No.".to_owned())));
    assert!(failed.ended);
    assert!(read_progress("begin\nwaiting winget\n").waiting_for_winget);
    assert!(!read_progress("waiting winget\nstart 1\n").waiting_for_winget);
}

#[test]
fn a_line_cut_short_or_not_ours_says_nothing() {
    assert_eq!(read_progress(""), Progress::default());
    assert_eq!(read_progress("sta\nok\nok x\nstart\nERROR: file in use\nfail\n"), Progress::default());
}

#[test]
fn what_a_guest_said_is_one_line_of_plain_text_and_its_end() {
    assert_eq!(one_line("a\r\nb\t\u{1b}[31mc"), "a b [31mc");
    let long = "x".repeat(1000) + "THE REASON";
    let said = one_line(&long);
    assert!(said.starts_with("...") && said.ends_with("THE REASON") && said.len() < 320, "{said}");
}

#[test]
fn windows_steps_go_to_the_guest_as_data_and_all_of_them_run() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&[
        "up",
        "started",
        "",
        "begin\nwaiting winget\n",
        "begin\nstart 1\n",
        "begin\nstart 1\nok 1\nstart 2\nok 2\nstart 3\n",
        "begin\nstart 1\nok 1\nstart 2\nok 2\nstart 3\nok 3\nend\n",
    ]));
    context.agent = agent.clone();
    let recipe = chosen("windows", &["Git.Git", "7zip.7zip"], Some(HOSTILE_SCRIPT));
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    stage(&mut task, &recipe).unwrap();
    assert_eq!(crate::recipe::read_outcome(&context, &vm), Outcome::Done);
    let data = data_sent(&agent);
    assert_eq!(
        data["steps"],
        serde_json::json!([
            {"number": 1, "kind": "package", "value": "Git.Git"},
            {"number": 2, "kind": "package", "value": "7zip.7zip"},
            {"number": 3, "kind": "script", "value": HOSTILE_SCRIPT},
        ])
    );
    let requests = agent.requests.lock().unwrap();
    for (_, _, script) in requests.iter() {
        assert!(!script.contains("Remove-Item"), "the script text is only ever Base64: {script}");
        assert!(!script.contains("Git.Git") || script.starts_with("$data = '"));
    }
    assert!(requests[1].2.ends_with("START\n") && requests[1].2.contains("$runner = '"));
    assert_eq!(base64_decode(requests[1].2.split('\'').nth(3).unwrap()).unwrap(), b"RUNNER\n");
    assert!(
        read_task(&context, None)
            .step
            .contains("Setting up for Box: Running the recipe's PowerShell script (3 of 3)")
    );
}

#[test]
fn a_step_that_fails_stops_the_rest_and_says_which_and_why() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    context.agent = Arc::new(ScriptedAgent::answering(&[
        "up",
        "started",
        "begin\nstart 1\nok 1\nstart 2\nfail 2 winget stopped with 5. Nothing matched.\nend\n",
    ]));
    let recipe = chosen("windows", &["Git.Git", "Nope.Nope"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    let stopped = stage(&mut task, &recipe).unwrap_err();
    assert_eq!(stopped.step, "Installing Nope.Nope");
    assert_eq!(stopped.why, "winget stopped with 5. Nothing matched.");
    let kept = crate::recipe::read_outcome(&context, &vm);
    assert_eq!(kept, Outcome::Failed { step: stopped.step, why: stopped.why });
}

#[test]
fn steps_that_say_nothing_for_an_hour_are_stuck() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&["up", "started"]));
    context.agent = agent;
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    let stopped = stage(&mut task, &recipe).unwrap_err();
    assert_eq!(stopped.step, "Starting the steps");
    assert!(stopped.why.contains("did not finish in 60 minutes"), "{}", stopped.why);
}

#[test]
fn a_guest_that_never_starts_the_steps_is_told_so() {
    let scratch = Scratch::new();
    let recipe = chosen("windows", &["Git.Git"], None);
    for (answers, wanted) in [
        (vec!["up", "nobody is logged in"], "Nobody is logged in"),
        (vec!["up", ""], "the agent went quiet"),
        (vec!["up", "Register-ScheduledTask : Access is denied"], "Access is denied"),
    ] {
        let mut context = windows_context(&scratch);
        context.agent = Arc::new(ScriptedAgent::answering(&answers));
        let vm = windows_machine(&context, &recipe);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "install", vm);
        assert!(stage(&mut task, &recipe).unwrap_err().why.contains(wanted), "{answers:?}");
    }
}

#[test]
fn an_agent_that_does_not_answer_in_five_minutes_is_given_up_on() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    context.agent = Arc::new(ScriptedAgent::default());
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    assert!(stage(&mut task, &recipe).unwrap_err().why.contains("agent did not answer"));
}

#[test]
fn a_machine_that_stops_during_the_steps_ends_them() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/recipe-start.ps1", "START\n");
    scratch.write("program/guest/recipe-run.ps1", "RUNNER\n");
    context.agent = Arc::new(ScriptedAgent::answering(&["up", "started"]));
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    assert!(stage(&mut task, &recipe).unwrap_err().why.contains("stopped while the steps were running"));
}

#[test]
fn a_recipe_with_nothing_to_run_is_done_and_asks_nothing_of_the_guest() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::default());
    context.agent = agent.clone();
    let recipe = chosen("windows", &[], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    stage(&mut task, &recipe).unwrap();
    assert_eq!(crate::recipe::read_outcome(&context, &vm), Outcome::Done);
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn a_machine_with_no_recipe_asks_the_guest_for_nothing() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::default());
    let shell = Arc::new(ScriptedShell::default());
    context.agent = agent.clone();
    context.shell = shell.clone();
    let vm = Machine { kind: Kind::Windows, ..machine(&context) };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    reset(&task);
    after_install(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty() && shell.asked().is_empty());
    assert_eq!(task.error, "");
    assert!(run_again(&mut task).unwrap_err().0.contains("no recipe"));
}

#[test]
fn a_failed_step_after_an_install_is_only_told() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    context.agent =
        Arc::new(ScriptedAgent::answering(&["up", "started", "begin\nstart 1\nfail 1 No.\nend\n"]));
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    task.error = "Only room for 8 GiB.".into();
    after_install(&mut task);
    assert!(
        task.error.starts_with("Only room for 8 GiB. The recipe Box stopped at \"Installing Git.Git\": No."),
        "{}",
        task.error
    );
    assert!(task.error.ends_with("Run the recipe again from the machine's settings."));
}

#[test]
fn a_machine_left_off_keeps_its_steps_waiting() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    let agent = Arc::new(ScriptedAgent::default());
    context.agent = agent.clone();
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    after_install(&mut task);
    assert!(task.error.contains("run at its first start"), "{}", task.error);
    assert_eq!(crate::recipe::read_outcome(&context, &vm), Outcome::Pending);
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn an_install_on_a_throwaway_leaves_its_steps_waiting_and_says_why() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::default());
    context.agent = agent.clone();
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = Machine { throwaway: true, ..windows_machine(&context, &recipe) };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    after_install(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty(), "nothing goes into a run that is thrown away");
    assert!(task.error.contains("a throwaway loses what they install"), "{}", task.error);
    assert_eq!(crate::recipe::read_outcome(&context, &vm), Outcome::Pending);
    assert!(crate::recipe::summary(&context, &vm).unwrap().ends_with("gone when it shuts down."));
}

#[test]
fn steps_an_install_left_waiting_run_at_the_first_start_once() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&["up", "started", "begin\nstart 1\nok 1\nend\n"]));
    context.agent = agent.clone();
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    after_start(&mut task);
    assert!(task.error.is_empty(), "{}", task.error);
    assert_eq!(crate::recipe::read_outcome(&context, &vm), Outcome::Done);
    assert!(read_task(&context, None).step.starts_with("Setting up for Box"));
    let asked = agent.requests.lock().unwrap().len();
    after_start(&mut task);
    assert_eq!(agent.requests.lock().unwrap().len(), asked, "never a second time");
}

#[test]
fn steps_a_start_cut_short_are_not_run_again_by_the_next_start() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    let agent = Arc::new(ScriptedAgent::answering(&["up", "started"]));
    context.agent = agent.clone();
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    crate::recipe::write_outcome(
        &context,
        &vm,
        &Outcome::Failed { step: CUT_SHORT_STEP.into(), why: CUT_SHORT.into() },
    )
    .unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    after_start(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty());
    let installing = Machine { installing: true, ..vm.clone() };
    crate::recipe::write_outcome(&context, &vm, &Outcome::Pending).unwrap();
    let mut task = Task::new(&context, &host, "start", installing);
    after_start(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty(), "not while the install is unfinished");
    let mut task = Task::new(&context, &host, "start", Machine { throwaway: true, ..vm });
    after_start(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty(), "nor on a throwaway, which would lose them");
}

#[test]
fn running_again_needs_a_machine_that_is_installed_and_running() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "recipe", vm.clone());
    assert!(run_again(&mut task).unwrap_err().0.contains("Start the machine first"));
    let mut installing = Task::new(&context, &host, "recipe", Machine { installing: true, ..vm });
    assert!(run_again(&mut installing).unwrap_err().0.contains("install has not finished"));
}

#[test]
fn running_again_runs_the_copy_the_user_agreed_to_and_a_failure_is_the_tasks() {
    let scratch = Scratch::new();
    let mut context = windows_context(&scratch);
    context.agent =
        Arc::new(ScriptedAgent::answering(&["up", "started", "begin\nstart 1\nfail 1 No.\nend\n"]));
    let recipe = chosen("windows", &["Git.Git"], None);
    let vm = windows_machine(&context, &recipe);
    // The file the recipe came from is gone, and what was agreed to is in the machine's folder.
    assert!(!std::path::Path::new(&recipe.file).exists());
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "recipe", vm);
    assert!(run_again(&mut task).unwrap_err().0.contains("stopped at \"Installing Git.Git\""));
}

fn ubuntu_context(scratch: &Scratch) -> Context {
    let context = windows_context(scratch);
    scratch.write("program/linux/recipe/run.sh", "RUNNER\n");
    scratch.write("program/linux/allow.txt", "");
    context
}

fn ubuntu_machine(context: &Context, chosen: &Chosen) -> Machine {
    let vm = Machine { kind: Kind::Linux, ..machine(context) };
    fs::create_dir_all(vm.folder(context).join("ssh")).unwrap();
    fs::write(vm.folder(context).join("ssh/id"), "key").unwrap();
    crate::recipe::write_chosen(context, &vm, chosen).unwrap();
    vm
}

fn ran(output: &str) -> ShellAnswer {
    ShellAnswer::Ran { status: 0, output: output.into() }
}

fn gone() -> ShellAnswer {
    ShellAnswer::Unreachable("Connection refused".into())
}

#[test]
fn ubuntu_steps_wait_for_the_first_boot_and_then_run_one_command_each_with_the_data_on_stdin() {
    let scratch = Scratch::new();
    let mut context = ubuntu_context(&scratch);
    let shell = Arc::new(ScriptedShell::answering(vec![
        gone(),
        ran(""),
        ShellAnswer::Ran { status: 1, output: String::new() },
        ran(""),
        ran(""),
        ran(""),
        ran("installed"),
        ran("installed"),
        ran("script ran"),
    ]));
    context.shell = shell.clone();
    let recipe = chosen("ubuntu", &["git", "build-essential"], Some(HOSTILE_SCRIPT));
    let vm = ubuntu_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    stage(&mut task, &recipe).unwrap();
    assert_eq!(crate::recipe::read_outcome(&context, &vm), Outcome::Done);
    let asked = shell.asked();
    let commands: Vec<&str> = asked.iter().map(|asked| asked.command.as_str()).collect();
    let folder = "/var/lib/guesthouse-recipe";
    assert_eq!(commands[..4], [ubuntu::PUT_RUNNER, ubuntu::PUT_RUNNER, ubuntu::READY, ubuntu::READY]);
    assert_eq!(
        commands[4..],
        [
            ubuntu::PUT_JOB,
            ubuntu::PREPARE,
            &format!("sh {folder}/run.sh run 1"),
            &format!("sh {folder}/run.sh run 2"),
            &format!("sh {folder}/run.sh run 3"),
        ]
    );
    assert_eq!(asked[0].input, b"RUNNER\n");
    let job = String::from_utf8(asked[4].input.clone()).unwrap();
    let lines: Vec<&str> = job.lines().collect();
    assert_eq!(lines.len(), 3, "a script with line breaks in it is still one line: {job}");
    assert_eq!(&lines[..2], ["step 1 apt git", "step 2 apt build-essential"]);
    let encoded = lines[2].strip_prefix("step 3 script ").unwrap();
    assert_eq!(base64_decode(encoded).unwrap(), HOSTILE_SCRIPT.as_bytes());
    for asked in &asked {
        assert!(
            !asked.command.contains("Remove-Item") && !asked.command.contains("evil"),
            "{}",
            asked.command
        );
    }
}

#[test]
fn an_ubuntu_step_that_fails_says_which_and_what_apt_said() {
    let scratch = Scratch::new();
    let mut context = ubuntu_context(&scratch);
    let failed = ShellAnswer::Ran {
        status: 100,
        output: "Reading package lists...\nE: Unable to locate package nope\n".into(),
    };
    context.shell =
        Arc::new(ScriptedShell::answering(vec![ran(""), ran(""), ran(""), ran(""), ran("ok"), failed]));
    let recipe = chosen("ubuntu", &["git", "nope"], None);
    let vm = ubuntu_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    let stopped = stage(&mut task, &recipe).unwrap_err();
    assert_eq!(stopped.step, "Installing nope");
    assert!(stopped.why.ends_with("E: Unable to locate package nope"), "{}", stopped.why);
    assert!(matches!(crate::recipe::read_outcome(&context, &vm), Outcome::Failed { .. }));
}

#[test]
fn a_guest_that_is_never_up_ends_the_wait_after_ninety_minutes() {
    let scratch = Scratch::new();
    let mut context = ubuntu_context(&scratch);
    let shell = Arc::new(ScriptedShell::default());
    context.shell = shell.clone();
    let recipe = chosen("ubuntu", &["git"], None);
    let vm = ubuntu_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    let stopped = stage(&mut task, &recipe).unwrap_err();
    assert!(stopped.why.contains("not ready in 90 minutes"), "{}", stopped.why);
    assert!(shell.asked().len() > 100, "it kept asking, a few seconds apart");
}

#[test]
fn an_ubuntu_machine_with_no_key_is_told_so_at_once() {
    let scratch = Scratch::new();
    let mut context = ubuntu_context(&scratch);
    let shell = Arc::new(ScriptedShell::default());
    context.shell = shell.clone();
    let recipe = chosen("ubuntu", &["git"], None);
    let vm = ubuntu_machine(&context, &recipe);
    fs::remove_file(vm.folder(&context).join("ssh/id")).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    assert!(stage(&mut task, &recipe).unwrap_err().why.contains("no SSH key"));
    assert!(shell.asked().is_empty());
}

#[test]
fn what_the_start_said_is_said_before_the_steps_begin_and_kept_at_the_end() {
    let scratch = Scratch::new();
    let mut context = ubuntu_context(&scratch);
    context.shell = Arc::new(ScriptedShell::answering(vec![ran(""), ran(""), ran(""), ran(""), ran("ok")]));
    let recipe = chosen("ubuntu", &["git"], None);
    let vm = ubuntu_machine(&context, &recipe);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    task.step("Starting the VM");
    let note = "Starting without the GPU: no nvfwd device.";
    task.error = note.into();
    after_install(&mut task);
    let said = task.said.lock().unwrap().clone();
    let at = |what: &str| {
        said.iter().position(|step| step.starts_with(what)).unwrap_or_else(|| panic!("{said:?}"))
    };
    assert!(at("Starting the VM") < at(note) && at(note) < at("Setting up for Box"), "{said:?}");
    assert_eq!(said.iter().filter(|step| *step == note).count(), 1, "{said:?}");
    assert_eq!(task.error, note, "the end still says it");
}
