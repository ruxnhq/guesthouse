use super::*;

fn recipe() -> Recipe {
    Recipe::parse(
        "name = \"Games\"\ndescription = \"For play.\"\nguest = \"windows\"\n[settings]\ncores = 6\nvram_gib = 0\n\
         [steps]\npackages = [\"Valve.Steam\"]\nscript = '''\nWrite-Output 1\nWrite-Output 2\n'''\n",
    )
    .unwrap()
}

#[test]
fn it_says_where_the_recipe_is_from() {
    assert_eq!(origin_text(Source::App, "/x"), "It comes with Guesthouse.");
    assert!(
        origin_text(Source::Yours, "/home/me/games.toml")
            .contains("your own recipe, from /home/me/games.toml")
    );
}

#[test]
fn it_shows_every_step_and_the_whole_script() {
    let text = what_it_will_run(&recipe(), Source::Yours, "/home/me/games.toml");
    assert!(text.starts_with("Games: For play.\nIt is your own recipe"), "{text}");
    assert!(text.contains("It sets 6 cores, no cap on video memory."), "{text}");
    assert!(text.contains("as the account's administrator"), "{text}");
    assert!(text.contains("- Install the winget package Valve.Steam"), "{text}");
    assert!(text.contains("- Run this PowerShell script:\n    Write-Output 1\n    Write-Output 2"), "{text}");
}

#[test]
fn a_recipe_with_no_steps_says_it_runs_nothing() {
    let recipe = Recipe::parse("name = \"Clean\"\ndescription = \"Bare.\"\nguest = \"ubuntu\"\n").unwrap();
    let text = what_it_will_run(&recipe, Source::App, "");
    assert!(text.ends_with("It runs nothing in the machine."), "{text}");
    assert!(!text.contains("It sets"));
}

#[test]
fn an_ubuntu_recipe_runs_as_root() {
    let recipe = Recipe::parse(
        "name = \"C\"\ndescription = \"D.\"\nguest = \"ubuntu\"\n[steps]\npackages = [\"git\"]\n",
    )
    .unwrap();
    assert!(what_it_will_run(&recipe, Source::App, "").contains("as root:\n- Install the apt package git"));
}

#[test]
fn a_machines_recipe_is_said_with_how_its_steps_went() {
    use crate::recipe::{Chosen, Outcome, write_chosen, write_outcome};
    use crate::testing::{Scratch, scratch_context};
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = Machine::new("box", "Box", "/d", "");
    std::fs::create_dir_all(vm.folder(&context)).unwrap();
    assert_eq!(summary(&context, &vm), None);
    write_chosen(&context, &vm, &Chosen { source: Source::App, file: String::new(), recipe: recipe() })
        .unwrap();
    assert_eq!(summary(&context, &vm).unwrap(), "Games (from Guesthouse). Its steps have not run yet.");
    write_outcome(
        &context,
        &vm,
        &Outcome::Failed { step: "Installing Valve.Steam".into(), why: "No.".into() },
    )
    .unwrap();
    assert!(
        summary(&context, &vm).unwrap().ends_with("Its steps stopped at \"Installing Valve.Steam\": No.")
    );
    write_outcome(&context, &vm, &Outcome::Done).unwrap();
    assert!(summary(&context, &vm).unwrap().ends_with("Its steps ran."));
}

#[test]
fn a_script_cannot_hide_part_of_itself_from_the_person_reading_it() {
    let recipe = Recipe::parse(
        "name = \"A\"\ndescription = \"B\"\nguest = \"ubuntu\"\n[steps]\nscript = \"echo hi\\u001b[2K\\rrm -rf x\\u202e\\tok\"\n",
    )
    .unwrap();
    let text = what_it_will_run(&recipe, Source::Yours, "/x.toml");
    assert!(text.contains("echo hi\\u{1b}[2K\\u{d}rm -rf x\\u{202e}\tok"), "{text}");
    assert!(!text.chars().any(|c| c == '\u{1b}' || c == '\r' || c == '\u{202e}'));
}
