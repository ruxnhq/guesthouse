//! `guesthouse-vm` and recipes: choosing one, seeing it first, and what each choice does to a machine.

use super::making_tests::{AMD_IGPU, failure_of, plain_disc, with_an_amd_card};
use super::*;
use crate::iso::stand_in::{make_iso, wim};
use crate::machine::load;
use crate::setup::amd_driver::stand_in::make_installer;

const GAMES: &str = "name = \"Games\"\ndescription = \"Windows for play.\"\nguest = \"windows\"\n\
                     [settings]\ncores = 3\nram_gib = 4\ndisk_gib = 100\nvram_gib = 0\n\
                     [steps]\npackages = [\"Valve.Steam\"]\nscript = \"Write-Output 1\"\n";
const CUDA: &str = "name = \"CUDA\"\ndescription = \"Ubuntu for CUDA.\"\nguest = \"ubuntu\"\n\
                    [steps]\npackages = [\"build-essential\"]\n";

fn with_recipes(scratch: &Scratch) -> Context {
    let mut context = with_an_amd_card(scratch);
    // A PC with eight cores, whatever the one the test runs on has, so a recipe's cores fit.
    context.cpus = crate::host::stand_in::fake_cpus(scratch, "cpus", &[32768; 16]);
    scratch.write("program/recipes/games.toml", GAMES);
    scratch.write("program/recipes/cuda.toml", CUDA);
    context
}

/// The words that make a Windows machine on the test's AMD card, with its driver.
fn windows_words(scratch: &Scratch, name: &str) -> String {
    let data = wim(&["Windows 11 Pro"], 26300, 9);
    let disc = make_iso(scratch, "win", &[("sources/install.wim", &data)]);
    let installer = make_installer(scratch, &[0x7550]);
    format!("new {name} --iso {disc} --amd-card {AMD_IGPU} --amd-driver {installer}")
}

#[test]
fn the_recipes_are_listed_with_what_each_is_for_and_the_files_that_are_not_recipes() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    scratch.write("home/.config/guesthouse/recipes/broken.toml", "name = 1");
    let (out, result) = said(&context, "recipes");
    assert_eq!(result, Ok(()));
    assert!(
        out.contains("app/games") && out.contains("Windows") && out.contains("Windows for play."),
        "{out}"
    );
    assert!(out.contains("app/cuda") && out.contains("Ubuntu"), "{out}");
    assert!(out.contains("Not a recipe, yours/broken.toml:"), "{out}");
    assert!(!out.contains("your own"), "the app's own are not said to be yours: {out}");
    scratch.write("home/.config/guesthouse/recipes/mine.toml", GAMES.replace("Games", "Mine"));
    let (out, _) = said(&context, "recipes");
    let mine = out.lines().find(|line| line.starts_with("yours/mine")).unwrap();
    assert!(mine.ends_with(", your own: what it runs is shown in full before it is used"), "{mine}");
    let (none, _) = said(&with_an_amd_card(&Scratch::new()), "recipes");
    assert!(none.starts_with("There are no recipes. Your own go in "), "{none}");
}

#[test]
fn a_machine_set_up_for_a_recipe_is_shown_it_first_and_takes_its_settings_unless_told_otherwise() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    let (out, result) = said(&context, &format!("{} --set-up-for games", windows_words(&scratch, "Play")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(out.contains("Games: Windows for play.\nIt comes with Guesthouse.\nIt sets 3 cores, 4 GiB of RAM, a 100 GiB disk"), "{out}");
    assert!(out.contains("- Install the winget package Valve.Steam"), "{out}");
    assert!(out.contains("- Run this PowerShell script:\n    Write-Output 1"), "{out}");
    assert!(out.contains("Its steps run once it is installed and up: Games."), "{out}");
    let made = load(&context, "play").unwrap();
    assert_eq!((made.cores, made.ram_gib), (3, 4));
    let chosen = crate::recipe::read_chosen(&context, &made).unwrap();
    assert_eq!(chosen.recipe.name, "Games");
    let (shown, _) = said(&context, "show play");
    assert!(shown.contains("recipe    Games (from Guesthouse). Its steps have not run yet."), "{shown}");
    let (typed, result) =
        said(&context, &format!("{} --set-up-for games --cores 2", windows_words(&scratch, "Typed")));
    assert_eq!(result, Ok(()), "{typed}");
    assert_eq!(load(&context, "typed").unwrap().cores, 2, "what was typed wins");
    assert_eq!(load(&context, "typed").unwrap().ram_gib, 4, "and the rest is the recipe's");
}

#[test]
fn a_machine_set_up_for_nothing_is_made_as_it_always_was() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    let (out, result) = said(&context, &windows_words(&scratch, "Oldway"));
    assert_eq!(result, Ok(()), "{out}");
    assert!(!out.contains("recipe") && !out.contains("Its steps"), "{out}");
    let made = load(&context, "oldway").unwrap();
    assert!(crate::recipe::read_chosen(&context, &made).is_none());
    assert!(!made.folder(&context).join("recipe.json").exists());
    assert!(!said(&context, "show oldway").0.contains("recipe"));
}

#[test]
fn a_recipe_of_the_users_own_that_runs_something_needs_yes_after_it_is_shown() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    scratch.write("home/.config/guesthouse/recipes/mine.toml", GAMES.replace("Games", "Mine"));
    let words = format!("{} --set-up-for yours/mine", windows_words(&scratch, "Own"));
    let (out, result) = said(&context, &words);
    let refusal = failure_of(result);
    assert!(refusal.contains("your own recipe") && refusal.contains("--yes"), "{refusal}");
    assert!(
        out.contains("It is your own recipe, from ") && out.contains("Write-Output 1"),
        "shown before it is refused: {out}"
    );
    assert!(load_all(&context).0.is_empty(), "nothing was made");
    let (_, result) = said(&context, &format!("{words} --yes"));
    assert_eq!(result, Ok(()));
    let chosen = crate::recipe::read_chosen(&context, &load(&context, "own").unwrap()).unwrap();
    assert_eq!(chosen.source, crate::recipe::Source::Yours);
}

#[test]
fn a_recipe_of_the_users_that_runs_nothing_needs_no_yes() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    scratch.write(
        "home/.config/guesthouse/recipes/small.toml",
        "name = \"Small\"\ndescription = \"Few cores.\"\nguest = \"windows\"\n[settings]\ncores = 2\n",
    );
    let (out, result) = said(&context, &format!("{} --set-up-for small", windows_words(&scratch, "Little")));
    assert_eq!(result, Ok(()), "{out}");
    assert_eq!(load(&context, "little").unwrap().cores, 2);
}

#[test]
fn a_recipe_that_cannot_be_used_is_refused_and_nothing_is_made() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    let windows = windows_words(&scratch, "W");
    let refused = |words: &str| failure_of(said(&context, words).1);
    let other_kind = refused(&format!("{windows} --set-up-for cuda"));
    assert!(
        other_kind.starts_with("There is no recipe called cuda for this machine. There are app/games"),
        "{other_kind}"
    );
    assert!(refused(&format!("{windows} --set-up-for nothing")).contains("no recipe called nothing"));
    let plain = format!("new P --iso {} --set-up-for games", plain_disc(&scratch));
    assert_eq!(refused(&plain), "A plain VM has no recipes. It is installed by hand.");
    assert_eq!(refused(&format!("{windows} --yes")), "--yes goes with --set-up-for.");
    assert_eq!(
        refused("new Mac --kind macos --set-up-for games"),
        "A macOS machine takes --disk, --cores and --ram, and nothing else."
    );
    assert!(load_all(&context).0.is_empty());
}

#[test]
fn what_a_recipe_asks_beyond_this_pc_comes_down_and_says_so() {
    let scratch = Scratch::new();
    let mut context = with_recipes(&scratch);
    context.ram_limit = Some(6);
    scratch.write(
        "program/recipes/games.toml",
        GAMES.replace("ram_gib = 4", "ram_gib = 64").replace("cores = 3", "cores = 4000"),
    );
    let (out, result) = said(&context, &format!("{} --set-up-for games", windows_words(&scratch, "Big")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(out.contains("Note: The recipe asks for 64 GiB of RAM, and this PC allows 6"), "{out}");
    assert!(out.contains("Note: The recipe asks for 4000 cores"), "{out}");
    assert_eq!(load(&context, "big").unwrap().ram_gib, 6);
}

#[test]
fn running_a_recipe_again_needs_one_and_a_running_machine() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    let bare = make(&context, "bare", "Bare", Kind::Windows);
    assert_eq!(failure_of(said(&context, "recipe bare").1), "Bare has no recipe.");
    let chosen = crate::recipe::Chosen {
        source: crate::recipe::Source::App,
        file: String::new(),
        recipe: crate::recipe::Recipe::parse(GAMES).unwrap(),
    };
    crate::recipe::write_chosen(&context, &bare, &chosen).unwrap();
    assert_eq!(failure_of(said(&context, "recipe bare").1), "Bare is off. Start it first.");
    assert!(usage_of("recipe").contains("recipe needs a machine."));
    assert!(usage_of("recipes bare").contains("recipes takes no words, only options."));
}

#[test]
fn lean_windows_is_off_unless_asked_and_is_shown_in_full_first() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    let (out, result) = said(&context, &windows_words(&scratch, "Usual"));
    assert_eq!(result, Ok(()), "{out}");
    assert!(!out.contains("lean"), "{out}");
    assert!(!crate::lean::is_lean(&context, &load(&context, "usual").unwrap()));
    let (out, result) = said(&context, &format!("{} --lean on", windows_words(&scratch, "Lean")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(out.contains("(Microsoft.YourPhone)") && out.contains("It leaves alone:"), "{out}");
    assert!(out.contains("It is lean Windows."), "{out}");
    assert!(crate::lean::is_lean(&context, &load(&context, "lean").unwrap()));
    let (shown, _) = said(&context, "show lean");
    assert!(shown.contains("lean      on"), "{shown}");
    let (out, result) = said(&context, &format!("new Ub --iso {} --lean on", plain_disc(&scratch)));
    assert!(failure_of(result).contains("--lean is only for a Windows machine"), "{out}");
}

#[test]
fn a_recipe_that_turns_lean_on_says_so_and_the_user_can_turn_it_off() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    scratch.write(
        "program/recipes/slim.toml",
        GAMES.replace("Games", "Slim").replace("[settings]", "lean = true\n[settings]"),
    );
    let (out, result) = said(&context, &format!("{} --set-up-for slim", windows_words(&scratch, "Slim")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(out.contains("It starts the machine as lean Windows, which you can turn off."), "{out}");
    assert_eq!(out.matches("(Microsoft.YourPhone)").count(), 1, "the list is shown once: {out}");
    assert!(crate::lean::is_lean(&context, &load(&context, "slim").unwrap()));
    let (out, result) =
        said(&context, &format!("{} --set-up-for slim --lean off", windows_words(&scratch, "Full")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(!crate::lean::is_lean(&context, &load(&context, "full").unwrap()));
}

#[test]
fn folders_a_recipe_names_are_left_out_of_scanning_only_when_the_user_says_so() {
    let scratch = Scratch::new();
    let context = with_recipes(&scratch);
    let named =
        GAMES.replace("Games", "Lib").replace("[settings]", "scan_exclusions = ['D:\\Games']\n[settings]");
    scratch.write("program/recipes/lib.toml", named);
    let (out, result) = said(&context, &format!("{} --set-up-for lib", windows_words(&scratch, "Kept")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(
        out.contains("Defender's scanning can leave out, D:\\Games, and that is off unless you turn it on"),
        "{out}"
    );
    assert!(!out.contains("Defender leaves out"), "{out}");
    let kept = load(&context, "kept").unwrap();
    assert!(crate::scan_exclusions::read_exclusions(&context, &kept).is_empty());
    assert!(said(&context, "show kept").0.contains("defender  scans every folder"));
    let (out, result) =
        said(&context, &format!("{} --set-up-for lib --skip-scanning on", windows_words(&scratch, "Fast")));
    assert_eq!(result, Ok(()), "{out}");
    assert!(
        out.contains(crate::scan_exclusions::WHAT_IT_MEANS) && out.contains("Defender leaves out D:\\Games."),
        "{out}"
    );
    let (shown, _) = said(&context, "show fast");
    assert!(shown.contains("defender  leaves out D:\\Games"), "{shown}");
    let (out, result) = said(&context, &format!("{} --skip-scanning on", windows_words(&scratch, "Bare")));
    assert!(failure_of(result).contains("goes with a Windows recipe that names folders"), "{out}");
    let (out, result) =
        said(&context, &format!("{} --set-up-for games --skip-scanning on", windows_words(&scratch, "None")));
    assert!(failure_of(result).contains("names folders"), "{out}");
}
