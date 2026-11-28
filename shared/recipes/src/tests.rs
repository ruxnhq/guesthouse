//! The crate's own hold on what it does, against the same golden files as the Linux host's tests in
//! `app-core/tests/recipes.rs`, so that it is held on every host the crate builds on.

use super::*;
use std::fs;
use std::path::{Path, PathBuf};

const GAMES: &str = include_str!("../../../recipes/windows-games.toml");
const CLEAN_BOX: &str = include_str!("../../../recipes/windows-clean-box.toml");
const CUDA: &str = include_str!("../../../recipes/ubuntu-cuda.toml");
const SHIPPED: [(&str, &str); 3] =
    [("windows-games", GAMES), ("windows-clean-box", CLEAN_BOX), ("ubuntu-cuda", CUDA)];
const APP_FOLDER: &str = "/opt/guesthouse/recipes";
const YOUR_FILE: &str = "/srv/recipes/mine.toml";

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux/app-core/tests/recipes").join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("guesthouse-recipes-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).unwrap();
    folder
}

fn read(folder: &Path, file: &str) -> String {
    fs::read_to_string(folder.join(file)).unwrap()
}

#[test]
fn the_shipped_recipes_read_and_show_as_the_goldens_have_them() {
    for (name, text) in SHIPPED {
        let recipe = Recipe::parse(text).unwrap();
        let json = serde_json::to_string_pretty(&recipe).unwrap() + "\n";
        assert_eq!(json, golden(&format!("{name}.read.json")), "{name}");
        let file = format!("{APP_FOLDER}/{name}.toml");
        let app = what_it_will_run(&recipe, Source::App, &file) + "\n";
        assert_eq!(app, golden(&format!("{name}.preview.txt")), "{name}");
        let yours = what_it_will_run(&recipe, Source::Yours, YOUR_FILE) + "\n";
        assert_eq!(yours, golden(&format!("{name}.yours.txt")), "{name}");
    }
}

#[test]
fn a_recipe_with_everything_shows_as_the_golden_has_it() {
    let full = "name = \"Box\"\ndescription = \"A box.\"\nguest = \"windows\"\nlean = true\n\
                scan_exclusions = ['D:\\Games', 'E:\\Builds']\n[settings]\ncores = 6\nram_gib = 12\n\
                disk_gib = 200\nvram_gib = 4\n[steps]\npackages = [\"Valve.Steam\", \"7zip.7zip\"]\n\
                script = \"\"\"\nWrite-Output 'a'\n\tWrite-Output \\u200b'b'\\u001b[0m\n\n  \n\"\"\"\n";
    let full = Recipe::parse(full).unwrap();
    assert_eq!(what_it_will_run(&full, Source::Yours, YOUR_FILE) + "\n", golden("full.yours.txt"));
}

#[test]
fn a_machines_files_are_written_as_the_goldens_have_them() {
    let folder = folder("record");
    for (name, text) in SHIPPED {
        let chosen = Chosen {
            source: Source::App,
            file: format!("{APP_FOLDER}/{name}.toml"),
            recipe: Recipe::parse(text).unwrap(),
        };
        write_chosen(&folder, &chosen).unwrap();
        assert_eq!(read(&folder, CHOSEN_FILE), golden(&format!("{name}.recipe.json")), "{name}");
        assert_eq!(read(&folder, OUTCOME_FILE), golden("recipe-result.pending.json"));
        assert_eq!(read_chosen(&folder), Some(chosen));
    }
    write_outcome(&folder, &Outcome::Done).unwrap();
    assert_eq!(read(&folder, OUTCOME_FILE), golden("recipe-result.done.json"));
    let failed =
        Outcome::Failed { step: "Running the recipe's PowerShell script".into(), why: "said \"no\"".into() };
    write_outcome(&folder, &failed).unwrap();
    assert_eq!(read(&folder, OUTCOME_FILE), golden("recipe-result.failed.json"));
    assert_eq!(read_outcome(&folder), failed);
    fs::write(folder.join(OUTCOME_FILE), "not json").unwrap();
    assert_eq!(read_outcome(&folder), Outcome::Pending);
    fs::write(folder.join(CHOSEN_FILE), golden("windows-games.recipe.json").replace("\"Games\"", "\"\""))
        .unwrap();
    assert_eq!(read_chosen(&folder), None);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn how_a_machines_recipe_went_is_said_as_the_host_says_it() {
    let games = Chosen { source: Source::App, file: String::new(), recipe: Recipe::parse(GAMES).unwrap() };
    assert_eq!(
        origin_and_outcome(&games, &Outcome::Pending, false),
        "It comes with Guesthouse. Its steps have not run yet."
    );
    let failed = Outcome::Failed { step: "Installing Valve.Steam".into(), why: "winget said no.".into() };
    assert_eq!(
        summary(&games, &failed, true),
        "Games (from Guesthouse). Its steps stopped at \"Installing Valve.Steam\": winget said no. On a \
         throwaway, what they install is gone when it shuts down."
    );
    let clean =
        Chosen { source: Source::Yours, file: String::new(), recipe: Recipe::parse(CLEAN_BOX).unwrap() };
    assert_eq!(
        origin_and_outcome(&clean, &Outcome::Done, true),
        "It is your own recipe. It has no steps to run."
    );
    assert_eq!(summary(&clean, &Outcome::Done, false), "Clean box (your own). It has no steps to run.");
}

#[test]
fn a_recipe_that_cannot_be_used_is_refused_in_the_same_words() {
    let said = |text: &str| Recipe::parse(text).unwrap_err().to_string();
    assert_eq!(
        said("name = \"Box\"\nguest = \"windows\"\n"),
        "The recipe could not be read, on line 1: missing field `description`"
    );
    assert_eq!(
        said("name = \"Box\"\ndescription = \"A box.\"\nguest = \"ubuntu\"\nlean = true\n"),
        "lean is for a Windows recipe."
    );
    assert_eq!(
        said("name = \"Box\"\ndescription = \"A box.\"\nguest = \"ubuntu\"\n[steps]\npackages = [\"Git\"]\n"),
        "\"Git\" is not a name apt packages have. A name is letters, digits and . _ + - only."
    );
}

#[test]
fn the_shelf_lists_and_finds_in_the_folders_it_is_given() {
    let app = folder("shelf-app");
    let yours = folder("shelf-yours");
    let recipe =
        |name: &str| format!("name = \"{name}\"\ndescription = \"For {name}.\"\nguest = \"windows\"\n");
    fs::write(app.join("games.toml"), recipe("Games")).unwrap();
    fs::write(yours.join("games.toml"), recipe("Games")).unwrap();
    fs::write(yours.join("bad.toml"), "name = ").unwrap();
    let listing = list(&app, &yours);
    let keys: Vec<&str> = listing.entries.iter().map(|entry| entry.key.as_str()).collect();
    assert_eq!(keys, ["app/games", "yours/games"]);
    assert_eq!(
        listing.problems,
        ["yours/bad.toml: The recipe could not be read, on line 1: string values must be quoted, expected \
          literal string"]
    );
    assert_eq!(find(list(&app, &yours), "yours/games").unwrap().source, Source::Yours);
    assert_eq!(
        find(list(&app, &yours), "games").unwrap_err().to_string(),
        "More than one recipe is called games: app/games, yours/games. Name one of them."
    );
    assert_eq!(
        find(Listing::default(), "games").unwrap_err().to_string(),
        "There is no recipe called games for this machine. There are none for this machine."
    );
    fs::remove_dir_all(app).unwrap();
    fs::remove_dir_all(yours).unwrap();
}

#[test]
fn settings_are_fitted_to_the_limits_given() {
    let limits = Limits {
        least_cores: 2,
        most_cores: 8,
        least_ram_gib: 4,
        most_ram_gib: 24,
        least_disk_gib: 40,
        most_disk_gib: 16384,
        most_vram_gib: 8,
    };
    let usual = Usual { cores: 4, ram_gib: 8, disk_gib: 64 };
    let asked =
        SettingsAsked { cores: Some(12), ram_gib: Some(64), disk_gib: Some(20_000), vram_gib: Some(12) };
    let big = fit(&asked, usual, limits);
    assert_eq!((big.cores, big.ram_gib, big.disk_gib, big.vram_gib), (8, 24, 16384, 8));
    assert_eq!(
        big.notes,
        [
            "The recipe asks for 12 cores, and this PC allows 8, so the machine has 8.",
            "The recipe asks for 64 GiB of RAM, and this PC allows 24, so the machine has 24.",
            "The recipe asks for 20000 GiB of disk, and this PC allows 16384, so the machine has 16384.",
            "The recipe asks for 12 GiB of video memory, and this PC allows 8, so the machine has 8."
        ]
    );
    let out_of_reach = fit(&SettingsAsked::default(), Usual { cores: 99, ram_gib: 99, disk_gib: 1 }, limits);
    assert_eq!((out_of_reach.cores, out_of_reach.ram_gib, out_of_reach.disk_gib), (8, 24, 40));
    assert!(out_of_reach.notes.is_empty());
    let upside_down = Limits { most_cores: 1, ..limits };
    assert_eq!(fit(&SettingsAsked::default(), usual, upside_down).cores, 2);
}

#[test]
fn lean_windows_says_and_keeps_what_the_goldens_have() {
    assert_eq!(lean::what_it_does() + "\n", golden("lean-what-it-does.txt"));
    assert_eq!(lean::guest_data() + "\n", golden("lean-on-disc.json"));
    let folder = folder("lean");
    assert!(!lean::is_lean(&folder));
    lean::write_lean(&folder, true).unwrap();
    assert_eq!(read(&folder, lean::LEAN_FILE), golden("lean.json"));
    assert!(lean::is_lean(&folder));
    lean::write_lean(&folder, false).unwrap();
    assert!(!folder.join(lean::LEAN_FILE).exists());
    let outcome = lean::Outcome {
        taken_out: vec!["MSTeams".into(), "Microsoft.BingNews".into()],
        put_back: vec!["Microsoft.Windows.DevHome".into()],
        still_back: Some("in use".into()),
    };
    lean::write_outcome(&folder, &outcome).unwrap();
    assert_eq!(read(&folder, lean::RESULT_FILE), golden("lean-result.json"));
    assert_eq!(lean::read_outcome(&folder), outcome);
    assert_eq!(
        lean::state_text(&outcome).unwrap(),
        "Took out Teams (free), News, which this Windows had. Windows had put back Dev Home, and it could not be \
         taken out again: in use"
    );
    assert_eq!(lean::state_text(&lean::Outcome::default()), None);
    fs::write(folder.join(lean::LEAN_FILE), "{\"lean\":true}").unwrap();
    fs::write(
        folder.join(lean::RESULT_FILE),
        r#"{"taken_out": ["Evil.App", "MSTeams", "Microsoft.BingNews"], "put_back": ["Microsoft.Windows.DevHome"], "still_back": "said"}"#,
    )
    .unwrap();
    lean::remake_imported(&folder).unwrap();
    assert_eq!(read(&folder, lean::LEAN_FILE), golden("lean.json"));
    assert_eq!(read(&folder, lean::RESULT_FILE), golden("lean-result.imported.json"));
    fs::remove_dir_all(folder).unwrap();
}

/// The answer file's step, in `guesthouse-answers-core`, runs the script and reads the list by the names
/// the disc is given them under here.
#[test]
fn the_answer_files_step_runs_what_the_disc_holds() {
    let step = guesthouse_answers_core::linux_host::lean::SPECIALIZE_COMMAND;
    assert!(step.contains(&format!(":\\{} -Data %i:\\{}", lean::SCRIPT, lean::DATA_ON_DISC)), "{step}");
}
