use super::*;
use crate::testing::{Scratch, scratch_context};

fn recipe(name: &str, guest: &str) -> String {
    format!("name = \"{name}\"\ndescription = \"For {name}.\"\nguest = \"{guest}\"\n")
}

#[test]
fn the_apps_recipes_and_the_users_are_listed_apart() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/recipes/games.toml", recipe("Games", "windows"));
    scratch.write("home/.config/guesthouse/recipes/mine.toml", recipe("Mine", "windows"));
    let listing = list(&context);
    let keys: Vec<(&str, Source)> =
        listing.entries.iter().map(|entry| (entry.key.as_str(), entry.source)).collect();
    assert_eq!(keys, [("app/games", Source::App), ("yours/mine", Source::Yours)]);
    assert!(listing.problems.is_empty());
}

#[test]
fn no_folder_is_no_recipes() {
    let scratch = Scratch::new();
    let listing = list(&scratch_context(&scratch));
    assert!(listing.entries.is_empty() && listing.problems.is_empty());
}

#[test]
fn a_bad_file_is_a_problem_with_its_name_and_the_rest_are_still_listed() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("home/.config/guesthouse/recipes/good.toml", recipe("Good", "ubuntu"));
    scratch.write("home/.config/guesthouse/recipes/bad.toml", "name = \"x\"\nrun = \"rm\"\n");
    scratch.write("home/.config/guesthouse/recipes/binary.toml", [0xff, 0xfe, 0x00]);
    scratch.write("home/.config/guesthouse/recipes/notes.txt", "not a recipe");
    scratch.write("home/.config/guesthouse/recipes/huge.toml", "#".repeat(200_000));
    let listing = list(&context);
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.problems.len(), 3, "{:?}", listing.problems);
    assert!(listing.problems.iter().any(|problem| problem.starts_with("yours/bad.toml:")));
    assert!(listing.problems.iter().any(|problem| problem.contains("yours/binary.toml: It is not text")));
}

#[test]
fn a_folder_named_like_a_recipe_is_no_recipe() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    std::fs::create_dir_all(scratch.path("home/.config/guesthouse/recipes/odd.toml")).unwrap();
    let listing = list(&context);
    assert!(listing.entries.is_empty());
    assert_eq!(listing.problems.len(), 1);
}

#[test]
fn a_machine_is_offered_the_recipes_for_its_kind_only() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/recipes/games.toml", recipe("Games", "windows"));
    scratch.write("program/recipes/cuda.toml", recipe("CUDA", "ubuntu"));
    let names = |kind| -> Vec<String> {
        list_for(&context, kind).entries.into_iter().map(|entry| entry.recipe.name).collect()
    };
    assert_eq!(names(Kind::Windows), ["Games"]);
    assert_eq!(names(Kind::Linux), ["CUDA"]);
    assert!(names(Kind::Plain).is_empty());
}

#[test]
fn a_recipe_is_found_by_its_key_its_file_or_its_name() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/recipes/games.toml", recipe("Games Box", "windows"));
    for wanted in ["app/games", "games", "GAMES", "games box", " Games Box "] {
        assert_eq!(find(&context, Kind::Windows, wanted).unwrap().key, "app/games", "{wanted}");
    }
}

#[test]
fn two_recipes_of_one_name_are_told_apart_by_their_keys() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/recipes/games.toml", recipe("Games", "windows"));
    scratch.write("home/.config/guesthouse/recipes/games.toml", recipe("Games", "windows"));
    let said = find(&context, Kind::Windows, "games").unwrap_err().0;
    assert!(said.contains("app/games, yours/games"), "{said}");
    assert_eq!(find(&context, Kind::Windows, "yours/games").unwrap().source, Source::Yours);
}

#[test]
fn a_recipe_that_is_not_there_says_which_are() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/recipes/games.toml", recipe("Games", "windows"));
    let said = find(&context, Kind::Windows, "nothing").unwrap_err().0;
    assert!(said.contains("app/games"), "{said}");
    let none = find(&context, Kind::Linux, "games").unwrap_err().0;
    assert!(none.contains("There are none for this machine"), "{none}");
}

/// What comes with the app is read by the app's own rules, so a file there that fails them is found
/// here and not by a user.
#[test]
fn every_recipe_that_comes_with_the_app_is_a_good_one() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths = crate::paths::Paths::new(
        &crate::paths::Variables::of(&[
            ("HOME", &scratch.path("home").display().to_string()),
            ("GUESTHOUSE_PROGRAM", concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        ]),
        None,
    );
    let listing = list(&context);
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);
    assert!(listing.entries.iter().all(|entry| entry.source == Source::App));
    // Each one is its own file, so any can go without this test caring which.
    assert!(listing.entries.iter().all(|entry| entry.key.starts_with("app/")));
}
