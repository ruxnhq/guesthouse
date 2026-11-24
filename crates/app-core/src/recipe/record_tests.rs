use super::*;
use crate::testing::{Scratch, scratch_context};
use std::fs;

fn machine(context: &Context) -> Machine {
    let machine = Machine::new("box", "Box", "/d", "");
    fs::create_dir_all(machine.folder(context)).unwrap();
    machine
}

fn chosen() -> Chosen {
    let recipe = Recipe::parse(
        "name = \"A\"\ndescription = \"B\"\nguest = \"windows\"\n[steps]\npackages = [\"Git.Git\"]\n",
    )
    .unwrap();
    Chosen { source: Source::Yours, file: "/home/me/a.toml".into(), recipe }
}

#[test]
fn a_machine_with_no_recipe_has_none() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    assert_eq!(read_chosen(&context, &vm), None);
    assert_eq!(read_outcome(&context, &vm), Outcome::Pending);
}

#[test]
fn the_recipe_is_kept_as_it_was_chosen_and_starts_pending() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    write_outcome(&context, &vm, &Outcome::Done).unwrap();
    write_chosen(&context, &vm, &chosen()).unwrap();
    assert_eq!(read_chosen(&context, &vm), Some(chosen()));
    assert_eq!(read_outcome(&context, &vm), Outcome::Pending, "a new choice has not run");
}

#[test]
fn how_it_went_is_kept() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    let failed = Outcome::Failed { step: "Installing Git.Git".into(), why: "winget is not there".into() };
    for outcome in [Outcome::Done, failed] {
        write_outcome(&context, &vm, &outcome).unwrap();
        assert_eq!(read_outcome(&context, &vm), outcome);
    }
}

#[test]
fn a_file_that_fails_the_checks_is_no_recipe() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    write_chosen(&context, &vm, &chosen()).unwrap();
    let path = vm.folder(&context).join(CHOSEN_FILE);
    let text = fs::read_to_string(&path).unwrap().replace("Git.Git", "a;b");
    fs::write(&path, text).unwrap();
    assert_eq!(read_chosen(&context, &vm), None);
    fs::write(&path, "{").unwrap();
    assert_eq!(read_chosen(&context, &vm), None);
}
