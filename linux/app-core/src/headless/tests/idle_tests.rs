use super::*;
use crate::agent::GuestAgent;
use crate::agent::make_token;
use crate::agent::stand_in::ScriptedAgent;
use crate::clock::stand_in::Ticking;
use crate::idle::write_guest_stat;
use std::sync::Arc;

const NOTHING: &str =
    "p|DISPLAY:\np|None.\np|\np|SYSTEM:\np|None.\np|\np|AWAYMODE:\np|None.\np|\np|EXECUTION:\np|None.\n";

fn quiet_answer() -> String {
    format!("sessions ok\ns|0|4|Services\ns|1|1|Console\npower ok\n{NOTHING}input 7200\n")
}

fn refused(context: &Context, words: &str) -> String {
    match said(context, words).1 {
        Err(Problem::Failed(failure)) => failure.0,
        other => panic!("{words:?} gave {other:?}"),
    }
}

/// A machine that runs as pid 40, with its guest time at a tick count, in a PC whose clock only moves
/// when it is slept on and whose agent answers from a list.
fn rig(scratch: &Scratch, kind: Kind, answers: &[&str]) -> (Context, Machine) {
    let mut context = scratch_context(scratch);
    context.clock = Arc::new(Ticking::default());
    context.agent = Arc::new(ScriptedAgent::answering(answers));
    scratch.write("program/guest/idle.ps1", "read it");
    scratch.write("program/guest/idle-input.ps1", "the helper");
    let machine = make(&context, "work", "Office PC", kind);
    make_token(&context, &machine).unwrap();
    running(&context, &machine, 40);
    write_guest_stat(&context, 40, 40, 1000);
    (context, machine)
}

#[test]
fn a_quiet_windows_machine_is_read_and_would_count_as_idle() {
    let scratch = Scratch::new();
    let (context, _) = rig(&scratch, Kind::Windows, &[&quiet_answer()]);
    let (out, result) = said(&context, "idle work");
    assert_eq!(result, Ok(()));
    assert!(out.contains("Last input       2 hours ago\n"), "{out}");
    assert!(out.contains("Remote sessions  none connected\n"), "{out}");
    assert!(out.contains("Power requests   none\n"), "{out}");
    assert!(out.contains("CPU time         0% of one core, over 2 seconds\n"), "{out}");
    assert!(out.contains("\nWould count as idle.\n"), "{out}");
    assert!(out.ends_with("Nothing was changed.\n"), "{out}");
}

#[test]
fn minutes_move_where_input_counts_as_quiet() {
    let scratch = Scratch::new();
    let (context, _) = rig(&scratch, Kind::Windows, &[&quiet_answer()]);
    let out = said(&context, "idle work --minutes 180").0;
    assert!(out.contains("Not idle.\nBusy: someone used the keyboard or mouse 2 hours ago."), "{out}");
    assert!(out.contains("Input counts as quiet after 3 hours."), "{out}");
    for words in ["idle work --minutes 0", "idle work --minutes soon"] {
        assert!(refused(&context, words).starts_with("--minutes is a number of minutes"), "{words}");
    }
}

#[test]
fn a_silent_agent_is_cannot_tell_and_not_idle() {
    let scratch = Scratch::new();
    let (context, _) = rig(&scratch, Kind::Windows, &[]);
    let out = said(&context, "idle work").0;
    assert!(out.contains("Last input       not read (the guest's agent did not answer)"), "{out}");
    assert!(out.contains("Cannot tell if it is idle."), "{out}");
    assert!(!out.contains("Would count as idle."), "{out}");
}

/// An agent whose guest runs while it is asked: the time the VM has had in the guest goes up.
struct Working(Context);

impl GuestAgent for Working {
    fn ask(&self, _: u16, _: &str, _: &str, _: u64) -> String {
        write_guest_stat(&self.0, 40, 40, 1000 + 800);
        String::new()
    }
}

#[test]
fn work_in_the_guest_is_busy_even_when_the_rest_could_not_be_read() {
    let scratch = Scratch::new();
    let (mut context, _) = rig(&scratch, Kind::Windows, &[]);
    let mut same = scratch_context(&scratch);
    same.processes = context.processes.clone();
    context.agent = Arc::new(Working(same));
    let out = said(&context, "idle work").0;
    // 800 ticks are 8 seconds of guest time, in the 2 seconds the reading takes.
    assert!(out.contains("CPU time         400% of one core, over 2 seconds\n"), "{out}");
    assert!(out.contains("Not idle.\nBusy: the guest is using 400% of a core."), "{out}");
}

#[test]
fn a_machine_that_is_off_has_nothing_to_read() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "work", "Office PC", Kind::Windows);
    assert_eq!(refused(&context, "idle work"), "Office PC is not running, so there is nothing to read.");
}

#[test]
fn another_kind_of_guest_says_what_it_cannot_give() {
    for (kind, name) in [(Kind::Linux, "an Ubuntu guest"), (Kind::Plain, "a plain VM")] {
        let scratch = Scratch::new();
        let (context, _) = rig(&scratch, kind, &[]);
        let out = said(&context, "idle work").0;
        assert!(out.contains(&format!("Last input       cannot be read for {name}\n")), "{out}");
        assert!(out.contains("CPU time         0% of one core"), "{out}");
        assert!(
            out.contains("Cannot tell if it is idle.\nOnly CPU time can be read for this kind of guest."),
            "{out}"
        );
    }
}

#[test]
fn the_words_after_idle_are_checked() {
    assert!(usage_of("idle").contains("idle needs a machine."));
    assert!(usage_of("idle a b").contains("idle takes only a machine."));
    assert!(usage_of("idle a --minutes").contains("--minutes needs a value."));
    assert!(usage_of("idle a --now").contains("idle has no option --now."));
}
