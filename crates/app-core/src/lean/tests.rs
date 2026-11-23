use super::*;
use crate::testing::{Scratch, scratch_context};

fn template() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../guest/autounattend.xml")).unwrap()
}

#[test]
fn the_choice_is_a_file_only_when_it_is_on() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = Machine::new("box", "Box", "/d", "");
    fs::create_dir_all(vm.folder(&context)).unwrap();
    assert!(!is_lean(&context, &vm));
    write_lean(&context, &vm, false).unwrap();
    assert!(!vm.folder(&context).join(LEAN_FILE).exists());
    write_lean(&context, &vm, true).unwrap();
    assert!(is_lean(&context, &vm));
    write_lean(&context, &vm, false).unwrap();
    assert!(!vm.folder(&context).join(LEAN_FILE).exists());
    fs::write(vm.folder(&context).join(LEAN_FILE), "{\"lean\": true, \"more\": 1}").unwrap();
    assert!(!is_lean(&context, &vm), "a file it can't read is no");
}

#[test]
fn the_step_goes_last_in_the_specialize_pass_and_nowhere_else() {
    let answers = template();
    let lean = with_specialize_step(&answers).unwrap();
    assert_eq!(lean.replace(SPECIALIZE_COMMAND, ""), answers, "only the one line is added");
    let specialize = lean.split("<settings pass=\"specialize\">").nth(1).unwrap();
    let specialize = specialize.split("</settings>").next().unwrap();
    assert!(specialize.contains(r"-File %i:\lean.ps1 -Data %i:\lean.json"));
    assert!(specialize.find("<Order>4</Order>").unwrap() < specialize.find("<Order>5</Order>").unwrap());
    assert_eq!(lean.matches("<Order>5</Order>").count(), 1);
}

#[test]
fn an_answer_file_with_no_place_for_it_is_refused() {
    assert!(with_specialize_step("<unattend/>").is_err());
}
