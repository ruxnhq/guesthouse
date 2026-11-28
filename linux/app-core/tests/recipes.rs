//! Recipes and lean Windows held as they are today, so that moving them can't change a byte: the
//! recipes that come with the app as they are read and shown, the files a machine keeps of them
//! (`recipe.json`, `recipe-result.json`, `lean.json`, `lean-result.json`) and the list a lean install's
//! disc carries, and every sentence a person reads of them. The golden files in `recipes/` were written
//! by today's code.

#![cfg(target_os = "linux")]

use guesthouse_app_core::context::Context;
use guesthouse_app_core::lean;
use guesthouse_app_core::machine::{Kind, Machine};
use guesthouse_app_core::recipe::*;
use guesthouse_app_core::testing::{Scratch, scratch_context};
use std::fs;
use std::path::Path;

const GAMES: &str = include_str!("../../../recipes/windows-games.toml");
const CLEAN_BOX: &str = include_str!("../../../recipes/windows-clean-box.toml");
const CUDA: &str = include_str!("../../../recipes/ubuntu-cuda.toml");
const SHIPPED: [(&str, &str); 3] =
    [("windows-games", GAMES), ("windows-clean-box", CLEAN_BOX), ("ubuntu-cuda", CUDA)];

/// Where the recipes of a made machine said they came from. No real PC's folder.
const APP_FOLDER: &str = "/opt/guesthouse/recipes";
const YOUR_FILE: &str = "/srv/recipes/mine.toml";

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/recipes").join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Today's text against the golden copy of it.
fn same(name: &str, text: impl AsRef<str>) {
    assert_eq!(text.as_ref(), golden(name), "{name}");
}

fn said(text: &str) -> String {
    Recipe::parse(text).unwrap_err().to_string()
}

fn windows(rest: &str) -> String {
    format!("name = \"Box\"\ndescription = \"A box.\"\nguest = \"windows\"\n{rest}")
}

fn ubuntu(rest: &str) -> String {
    format!("name = \"Box\"\ndescription = \"A box.\"\nguest = \"ubuntu\"\n{rest}")
}

fn machine(context: &Context) -> Machine {
    let vm = Machine::new("box", "Box", "/d", "");
    fs::create_dir_all(vm.folder(context)).unwrap();
    vm
}

fn read(vm: &Machine, context: &Context, file: &str) -> String {
    fs::read_to_string(vm.folder(context).join(file)).unwrap()
}

// The recipes that come with the app.

#[test]
fn each_shipped_recipe_reads_as_it_did() {
    for (name, text) in SHIPPED {
        let recipe = Recipe::parse(text).unwrap();
        let json = serde_json::to_string_pretty(&recipe).unwrap() + "\n";
        same(&format!("{name}.read.json"), json);
    }
}

#[test]
fn each_shipped_recipe_says_what_it_will_run_as_it_did() {
    for (name, text) in SHIPPED {
        let recipe = Recipe::parse(text).unwrap();
        let file = format!("{APP_FOLDER}/{name}.toml");
        same(&format!("{name}.preview.txt"), what_it_will_run(&recipe, Source::App, &file) + "\n");
        let yours = what_it_will_run(&recipe, Source::Yours, YOUR_FILE) + "\n";
        same(&format!("{name}.yours.txt"), yours);
    }
}

#[test]
fn each_shipped_recipe_names_its_steps_as_it_did() {
    let names: Vec<Vec<String>> =
        SHIPPED.iter().map(|(_, text)| Recipe::parse(text).unwrap().step_names()).collect();
    assert_eq!(names[0], ["Installing Valve.Steam"]);
    assert!(names[1].is_empty());
    assert_eq!(
        names[2],
        [
            "Installing build-essential",
            "Installing cmake",
            "Installing git",
            "Installing python3-pip",
            "Installing python3-venv",
            "Installing nvidia-cuda-toolkit"
        ]
    );
    let script = Recipe::parse(&ubuntu("[steps]\npackages = [\"git\"]\nscript = \"true\"\n")).unwrap();
    assert_eq!(script.step_names(), ["Installing git", "Running the recipe's shell script"]);
    let script = Recipe::parse(&windows("[steps]\nscript = \"exit 0\"\n")).unwrap();
    assert_eq!(script.step_names(), ["Running the recipe's PowerShell script"]);
}

#[test]
fn each_shipped_recipe_is_for_its_kind() {
    let kinds: Vec<Kind> = SHIPPED.iter().map(|(_, text)| Recipe::parse(text).unwrap().kind()).collect();
    assert_eq!(kinds, [Kind::Windows, Kind::Windows, Kind::Linux]);
    assert_eq!(Guest::Windows.kind(), Kind::Windows);
    assert_eq!(Guest::Ubuntu.kind(), Kind::Linux);
    assert_eq!(Guest::of_kind(Kind::Windows), Some(Guest::Windows));
    assert_eq!(Guest::of_kind(Kind::Linux), Some(Guest::Ubuntu));
    assert_eq!(Guest::of_kind(Kind::Plain), None);
    let words: Vec<(&str, &str, &str)> = [Guest::Windows, Guest::Ubuntu]
        .iter()
        .map(|guest| (guest.name(), guest.package_name(), guest.script_name()))
        .collect();
    assert_eq!(
        words,
        [("Windows", "winget package", "PowerShell script"), ("Ubuntu", "apt package", "shell script")]
    );
}

// Every sentence a recipe that can't be used is refused with.

#[test]
fn every_refusal_of_a_recipe_is_said_as_it_was() {
    let long_file = format!("{}\n# {}\n", windows(""), "x".repeat(64 * 1024));
    assert_eq!(said(&long_file), "A recipe is at most 64 KiB.");
    assert_eq!(
        said("name = \"Box\"\nguest = \"windows\"\n"),
        "The recipe could not be read, on line 1: missing field `description`"
    );
    assert_eq!(
        said(&windows("colour = \"red\"\n")),
        "The recipe could not be read, on line 4: unknown field `colour`, expected one of `name`, \
         `description`, `guest`, `settings`, `steps`, `lean`, `scan_exclusions`"
    );
    assert_eq!(
        said(&windows("[settings]\ncores = \"many\"\n")),
        "The recipe could not be read, on line 5: invalid type: string \"many\", expected u32"
    );
    assert_eq!(
        said("name = \"Box\"\ndescription = \"A box.\"\nguest = \"mac\"\n"),
        "The recipe could not be read, on line 3: unknown variant `mac`, expected `windows` or `ubuntu`"
    );
    assert_eq!(
        said("name = \n"),
        "The recipe could not be read, on line 1: string values must be quoted, expected literal string"
    );
    assert_eq!(
        said(&windows("[settings]\ngpu = 1\n")),
        "The recipe could not be read, on line 5: unknown field `gpu`, expected one of `cores`, `ram_gib`, \
         `disk_gib`, `vram_gib`"
    );
    assert_eq!(
        said(&windows("[steps]\nrun = \"x\"\n")),
        "The recipe could not be read, on line 5: unknown field `run`, expected `packages` or `script`"
    );
    let named = |name: &str| format!("name = \"{name}\"\ndescription = \"A box.\"\nguest = \"windows\"\n");
    assert_eq!(said(&named(" ")), "The recipe's name is empty.");
    assert_eq!(said(&named(&"n".repeat(41))), "The recipe's name is longer than 40 characters.");
    Recipe::parse(&named(&"n".repeat(40))).unwrap();
    assert_eq!(said(&named("Two\\nlines")), "The recipe's name must be one line of plain text.");
    assert_eq!(said(&named("Hidden\\u202emark")), "The recipe's name must be one line of plain text.");
    let described = |text: &str| format!("name = \"Box\"\ndescription = \"{text}\"\nguest = \"windows\"\n");
    assert_eq!(said(&described("")), "The recipe's description is empty.");
    assert_eq!(said(&described(&"d".repeat(161))), "The recipe's description is longer than 160 characters.");
    assert_eq!(said(&described("a\\u200bb")), "The recipe's description must be one line of plain text.");
    assert_eq!(said(&windows("[settings]\ncores = 0\n")), "cores must be more than 0.");
    assert_eq!(said(&windows("[settings]\nram_gib = 0\n")), "ram_gib must be more than 0.");
    assert_eq!(said(&windows("[settings]\ndisk_gib = 0\n")), "disk_gib must be more than 0.");
    assert_eq!(said(&windows("[settings]\ncores = 0\nram_gib = 0\n")), "cores must be more than 0.");
    Recipe::parse(&windows("[settings]\nvram_gib = 0\n")).unwrap();
    assert_eq!(
        said(&ubuntu("[settings]\nvram_gib = 2\n")),
        "vram_gib is for a Windows recipe: only Windows machines have the cap."
    );
    assert_eq!(said(&ubuntu("lean = true\n")), "lean is for a Windows recipe.");
    Recipe::parse(&ubuntu("lean = false\n")).unwrap();
    assert_eq!(
        said(&ubuntu("scan_exclusions = ['D:\\Games']\n")),
        "scan_exclusions is for a Windows recipe."
    );
    assert_eq!(
        said(&windows("scan_exclusions = ['C:\\Windows']\n")),
        "C:\\Windows holds too much of Windows, or what comes from the internet, to leave out of scanning."
    );
    let many: Vec<String> = (0..31).map(|n| format!("\"pkg{n}\"")).collect();
    assert_eq!(
        said(&ubuntu(&format!("[steps]\npackages = [{}]\n", many.join(", ")))),
        "A recipe lists at most 30 packages."
    );
    Recipe::parse(&ubuntu(&format!("[steps]\npackages = [{}]\n", many[..30].join(", ")))).unwrap();
}

#[test]
fn every_package_name_refused_is_said_as_it_was() {
    let package =
        |guest: fn(&str) -> String, name: &str| said(&guest(&format!("[steps]\npackages = [\"{name}\"]\n")));
    assert_eq!(
        package(windows, "-x"),
        "\"-x\" is not a name winget packages have. A name is letters, digits and . _ + - only."
    );
    assert_eq!(
        package(windows, "a b"),
        "\"a b\" is not a name winget packages have. A name is letters, digits and . _ + - only."
    );
    assert_eq!(
        package(ubuntu, "Git"),
        "\"Git\" is not a name apt packages have. A name is letters, digits and . _ + - only."
    );
    assert_eq!(
        package(ubuntu, "a_b"),
        "\"a_b\" is not a name apt packages have. A name is letters, digits and . _ + - only."
    );
    assert_eq!(
        package(ubuntu, "g"),
        "\"g\" is not a name apt packages have. A name is letters, digits and . _ + - only."
    );
    assert_eq!(
        package(windows, "a\\u0007b"),
        "\"ab\" is not a name winget packages have. A name is letters, digits and . _ + - only."
    );
    let long = "p".repeat(129);
    assert_eq!(
        package(ubuntu, &long),
        format!(
            "\"{}\" is not a name apt packages have. A name is letters, digits and . _ + - only.",
            &long[..128]
        )
    );
    for fine in ["Valve.Steam", "Microsoft.VisualStudio.2022.Community", "7zip.7zip", "a_b+c-d"] {
        Recipe::parse(&windows(&format!("[steps]\npackages = [\"{fine}\"]\n"))).unwrap();
    }
    for fine in ["g++", "python3.12", "libc6-dev", "p".repeat(128).as_str()] {
        Recipe::parse(&ubuntu(&format!("[steps]\npackages = [\"{fine}\"]\n"))).unwrap();
    }
}

#[test]
fn every_script_refused_is_said_as_it_was() {
    let script = format!("[steps]\nscript = '''\n{}\n'''\n", "x".repeat(32 * 1024));
    assert_eq!(said(&windows(&script)), "The script is longer than 32 KiB.");
    assert_eq!(
        said(&windows("[steps]\nscript = \"  \\n \"\n")),
        "The script is empty, or has a character no script has."
    );
    assert_eq!(
        said(&ubuntu("[steps]\nscript = \"echo \\u0000\"\n")),
        "The script is empty, or has a character no script has."
    );
    let fits = format!("[steps]\nscript = '''{}'''\n", "x".repeat(32 * 1024));
    Recipe::parse(&windows(&fits)).unwrap();
}

// The shelf.

fn shelf_recipe(name: &str, guest: &str) -> String {
    format!("name = \"{name}\"\ndescription = \"For {name}.\"\nguest = \"{guest}\"\n")
}

const APP: &str = "program/recipes";
const YOURS: &str = "home/.config/guesthouse/recipes";

#[test]
fn the_shelf_lists_and_refuses_as_it_did() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write(&format!("{APP}/games.toml"), shelf_recipe("Games", "windows"));
    scratch.write(&format!("{APP}/cuda.toml"), shelf_recipe("CUDA", "ubuntu"));
    scratch.write(&format!("{APP}/notes.txt"), "not a recipe");
    scratch.write(&format!("{YOURS}/bad.toml"), "name = ");
    scratch.write(&format!("{YOURS}/binary.toml"), [0xff, 0xfe, 0x00]);
    scratch.write(&format!("{YOURS}/big.toml"), "#".repeat(128 * 1024 + 1));
    scratch.write(&format!("{YOURS}/mine.toml"), shelf_recipe("Mine", "windows"));
    scratch.write(&format!("{YOURS}/plain.toml"), shelf_recipe("", "windows"));
    fs::create_dir_all(scratch.path(&format!("{YOURS}/odd.toml"))).unwrap();
    let listing = list(&context);
    let keys: Vec<(&str, Source, &str)> = listing
        .entries
        .iter()
        .map(|entry| (entry.key.as_str(), entry.source, entry.recipe.name.as_str()))
        .collect();
    assert_eq!(
        keys,
        [
            ("app/cuda", Source::App, "CUDA"),
            ("app/games", Source::App, "Games"),
            ("yours/mine", Source::Yours, "Mine")
        ]
    );
    assert_eq!(listing.entries[1].file, scratch.path(&format!("{APP}/games.toml")));
    assert_eq!(
        listing.problems,
        [
            "yours/bad.toml: The recipe could not be read, on line 1: string values must be quoted, expected \
             literal string",
            "yours/big.toml: It is not a recipe file.",
            "yours/binary.toml: It is not text.",
            "yours/odd.toml: It is not a recipe file.",
            "yours/plain.toml: The recipe's name is empty."
        ]
    );
    let names = |kind| -> (Vec<String>, usize) {
        let listing = list_for(&context, kind);
        (listing.entries.into_iter().map(|entry| entry.key).collect(), listing.problems.len())
    };
    assert_eq!(names(Kind::Windows), (vec!["app/games".to_owned(), "yours/mine".to_owned()], 5));
    assert_eq!(names(Kind::Linux), (vec!["app/cuda".to_owned()], 5));
    assert_eq!(names(Kind::Plain), (vec![], 5));
}

#[test]
fn the_shelf_finds_and_says_as_it_did() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write(&format!("{APP}/games.toml"), shelf_recipe("Games Box", "windows"));
    scratch.write(&format!("{APP}/office.toml"), shelf_recipe("Office", "windows"));
    scratch.write(&format!("{YOURS}/games.toml"), shelf_recipe("Mine", "windows"));
    scratch.write(&format!("{YOURS}/box.toml"), shelf_recipe("Games Box", "windows"));
    let found = |wanted: &str| find(&context, Kind::Windows, wanted).map(|entry| entry.key);
    let refused = |kind, wanted: &str| find(&context, kind, wanted).unwrap_err().to_string();
    assert_eq!(found("app/games").unwrap(), "app/games");
    assert_eq!(found(" YOURS/GAMES ").unwrap(), "yours/games");
    assert_eq!(found("office").unwrap(), "app/office");
    assert_eq!(found("Mine").unwrap(), "yours/games");
    assert_eq!(found("yours/box").unwrap(), "yours/box");
    assert_eq!(
        refused(Kind::Windows, "games"),
        "More than one recipe is called games: app/games, yours/games. Name one of them."
    );
    assert_eq!(
        refused(Kind::Windows, " Games BOX"),
        "More than one recipe is called games box: app/games, yours/box. Name one of them."
    );
    assert_eq!(
        refused(Kind::Windows, "Nothing"),
        "There is no recipe called nothing for this machine. There are app/games, app/office, yours/box, \
         yours/games."
    );
    assert_eq!(
        refused(Kind::Linux, "games"),
        "There is no recipe called games for this machine. There are none for this machine."
    );
    assert_eq!(
        refused(Kind::Plain, "games"),
        "There is no recipe called games for this machine. There are none for this machine."
    );
}

// What a recipe says it will do.

#[test]
fn where_a_recipe_is_from_is_said_as_it_was() {
    assert_eq!(origin_text(Source::App, "/x"), "It comes with Guesthouse.");
    assert_eq!(
        origin_text(Source::Yours, YOUR_FILE),
        "It is your own recipe, from /srv/recipes/mine.toml. Nobody else has looked at it."
    );
}

#[test]
fn every_part_of_a_preview_is_said_as_it_was() {
    let full = windows(
        "lean = true\nscan_exclusions = ['D:\\Games', 'E:\\Builds']\n[settings]\ncores = 6\nram_gib = 12\n\
         disk_gib = 200\nvram_gib = 4\n[steps]\npackages = [\"Valve.Steam\", \"7zip.7zip\"]\n\
         script = \"\"\"\nWrite-Output 'a'\n\tWrite-Output \\u200b'b'\\u001b[0m\n\n  \n\"\"\"\n",
    );
    let full = Recipe::parse(&full).unwrap();
    same("full.yours.txt", what_it_will_run(&full, Source::Yours, YOUR_FILE) + "\n");
    let capless = Recipe::parse(&windows("[settings]\nvram_gib = 0\n")).unwrap();
    assert_eq!(
        what_it_will_run(&capless, Source::App, ""),
        "Box: A box.\nIt comes with Guesthouse.\nIt sets no cap on video memory.\nIt runs nothing in the machine."
    );
    let bare = Recipe::parse(&ubuntu("")).unwrap();
    assert_eq!(
        what_it_will_run(&bare, Source::App, ""),
        "Box: A box.\nIt comes with Guesthouse.\n\nIt runs nothing in the machine."
    );
    let ubuntu_script =
        Recipe::parse(&ubuntu("[settings]\ndisk_gib = 50\n[steps]\nscript = \"apt-get -y upgrade\\n\"\n"))
            .unwrap();
    assert_eq!(
        what_it_will_run(&ubuntu_script, Source::App, ""),
        "Box: A box.\nIt comes with Guesthouse.\nIt sets a 50 GiB disk.\nOnce Ubuntu is installed and up, it \
         runs in the machine, as root:\n- Run this shell script:\n    apt-get -y upgrade"
    );
    let excluded = Recipe::parse(&windows("scan_exclusions = ['D:\\Games']\n")).unwrap();
    assert_eq!(
        what_it_will_run(&excluded, Source::App, ""),
        "Box: A box.\nIt comes with Guesthouse.\n\nIt names folders that Defender's scanning can leave out, \
         D:\\Games, and that is off unless you turn it on.\nIt runs nothing in the machine."
    );
}

fn made_with(context: &Context, vm: &Machine, text: &str, source: Source) {
    let chosen = Chosen { source, file: YOUR_FILE.to_owned(), recipe: Recipe::parse(text).unwrap() };
    write_chosen(context, vm, &chosen).unwrap();
}

#[test]
fn how_a_machines_recipe_went_is_said_as_it_was() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut vm = machine(&context);
    assert_eq!(origin_and_outcome(&context, &vm), None);
    assert_eq!(summary(&context, &vm), None);
    made_with(&context, &vm, GAMES, Source::App);
    assert_eq!(
        origin_and_outcome(&context, &vm).unwrap(),
        "It comes with Guesthouse. Its steps have not run yet."
    );
    assert_eq!(summary(&context, &vm).unwrap(), "Games (from Guesthouse). Its steps have not run yet.");
    write_outcome(&context, &vm, &Outcome::Done).unwrap();
    assert_eq!(origin_and_outcome(&context, &vm).unwrap(), "It comes with Guesthouse. Its steps ran.");
    let failed = Outcome::Failed { step: "Installing Valve.Steam".into(), why: "winget said no.".into() };
    write_outcome(&context, &vm, &failed).unwrap();
    assert_eq!(
        summary(&context, &vm).unwrap(),
        "Games (from Guesthouse). Its steps stopped at \"Installing Valve.Steam\": winget said no."
    );
    vm.throwaway = true;
    assert_eq!(
        origin_and_outcome(&context, &vm).unwrap(),
        "It comes with Guesthouse. Its steps stopped at \"Installing Valve.Steam\": winget said no. On a \
         throwaway, what they install is gone when it shuts down."
    );
    made_with(&context, &vm, CLEAN_BOX, Source::Yours);
    assert_eq!(origin_and_outcome(&context, &vm).unwrap(), "It is your own recipe. It has no steps to run.");
    assert_eq!(summary(&context, &vm).unwrap(), "Clean box (your own). It has no steps to run.");
    vm.throwaway = false;
    made_with(&context, &vm, CUDA, Source::Yours);
    assert_eq!(summary(&context, &vm).unwrap(), "CUDA (your own). Its steps have not run yet.");
}

// The files a machine keeps.

#[test]
fn recipe_json_and_recipe_result_json_are_written_as_they_were() {
    assert_eq!((CHOSEN_FILE, OUTCOME_FILE), ("recipe.json", "recipe-result.json"));
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    for (name, text) in SHIPPED {
        let chosen = Chosen {
            source: Source::App,
            file: format!("{APP_FOLDER}/{name}.toml"),
            recipe: Recipe::parse(text).unwrap(),
        };
        write_chosen(&context, &vm, &chosen).unwrap();
        same(&format!("{name}.recipe.json"), read(&vm, &context, CHOSEN_FILE));
        same("recipe-result.pending.json", read(&vm, &context, OUTCOME_FILE));
        assert_eq!(read_chosen(&context, &vm), Some(chosen));
    }
    made_with(&context, &vm, &windows("lean = true\n[steps]\nscript = \"x\"\n"), Source::Yours);
    same("yours.recipe.json", read(&vm, &context, CHOSEN_FILE));
    write_outcome(&context, &vm, &Outcome::Done).unwrap();
    same("recipe-result.done.json", read(&vm, &context, OUTCOME_FILE));
    let failed =
        Outcome::Failed { step: "Running the recipe's PowerShell script".into(), why: "said \"no\"".into() };
    write_outcome(&context, &vm, &failed).unwrap();
    same("recipe-result.failed.json", read(&vm, &context, OUTCOME_FILE));
    assert_eq!(read_outcome(&context, &vm), failed);
}

#[test]
fn recipe_json_and_recipe_result_json_are_read_as_they_were() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    let write = |file: &str, text: &str| fs::write(vm.folder(&context).join(file), text).unwrap();
    assert_eq!(read_outcome(&context, &vm), Outcome::Pending);
    for (text, outcome) in [
        (r#"{"state": "pending"}"#, Outcome::Pending),
        (r#"{"state": "done"}"#, Outcome::Done),
        (r#"{"state": "done", "more": 1}"#, Outcome::Done),
        (
            r#"{"state": "failed", "step": "s", "why": "w"}"#,
            Outcome::Failed { step: "s".into(), why: "w".into() },
        ),
        (r#"{"state": "failed", "step": "s"}"#, Outcome::Pending),
        (r#"{"state": "gone"}"#, Outcome::Pending),
        ("not json", Outcome::Pending),
    ] {
        write(OUTCOME_FILE, text);
        assert_eq!(read_outcome(&context, &vm), outcome, "{text}");
    }
    write(CHOSEN_FILE, &golden("windows-games.recipe.json"));
    assert_eq!(read_chosen(&context, &vm).unwrap().recipe.name, "Games");
    let bad = golden("windows-games.recipe.json").replace("\"Games\"", "\"\"");
    write(CHOSEN_FILE, &bad);
    assert_eq!(read_chosen(&context, &vm), None, "a recipe that fails its checks is none");
    let extra = golden("windows-games.recipe.json").replacen('{', "{\"more\": 1,", 1);
    write(CHOSEN_FILE, &extra);
    assert_eq!(
        read_chosen(&context, &vm).unwrap().recipe.name,
        "Games",
        "a field more around it is kept out"
    );
    let unknown =
        golden("windows-games.recipe.json").replace("\"lean\": true", "\"lean\": true, \"more\": 1");
    write(CHOSEN_FILE, &unknown);
    assert_eq!(read_chosen(&context, &vm), None, "a field more in the recipe is none");
    write(CHOSEN_FILE, "[]");
    assert_eq!(read_chosen(&context, &vm), None);
}

// A recipe's settings, fitted to this PC.

/// A PC with eight cores in sixteen CPUs and 32 GiB of RAM, whatever the one the test runs on has.
fn small_pc(scratch: &Scratch) -> Context {
    let mut context = scratch_context(scratch);
    for cpu in 0..16 {
        scratch.write(&format!("cpus/cpu{cpu}/topology/core_id"), format!("{}\n", cpu % 8));
        scratch.write(&format!("cpus/cpu{cpu}/topology/physical_package_id"), "0\n");
    }
    context.cpus = scratch.path("cpus");
    context.ram_limit = Some(24);
    context.meminfo = scratch.write("meminfo", "MemTotal:       33554432 kB\nMemFree:        1000 kB\n");
    context
}

fn fitted(fitted: &Fitted) -> (u32, u32, u32, u32) {
    (fitted.cores, fitted.ram_gib, fitted.disk_gib, fitted.vram_gib)
}

const USUAL: Usual = Usual { cores: 4, ram_gib: 8, disk_gib: 64 };

#[test]
fn a_recipes_settings_are_fitted_as_they_were() {
    let scratch = Scratch::new();
    let context = small_pc(&scratch);
    let asked = |cores, ram_gib, disk_gib, vram_gib| SettingsAsked { cores, ram_gib, disk_gib, vram_gib };
    let none = SettingsAsked::default();
    let usual = fit(&context, Kind::Windows, &none, USUAL, 8);
    assert_eq!((fitted(&usual), usual.notes.len()), ((4, 8, 64, 0), 0));
    let fine = fit(&context, Kind::Windows, &asked(Some(8), Some(24), Some(40), Some(8)), USUAL, 8);
    assert_eq!((fitted(&fine), fine.notes.len()), ((8, 24, 40, 8), 0));
    let big = fit(&context, Kind::Windows, &asked(Some(12), Some(64), Some(20_000), Some(12)), USUAL, 8);
    assert_eq!(fitted(&big), (8, 24, 16384, 8));
    assert_eq!(
        big.notes,
        [
            "The recipe asks for 12 cores, and this PC allows 8, so the machine has 8.",
            "The recipe asks for 64 GiB of RAM, and this PC allows 24, so the machine has 24.",
            "The recipe asks for 20000 GiB of disk, and this PC allows 16384, so the machine has 16384.",
            "The recipe asks for 12 GiB of video memory, and this PC allows 8, so the machine has 8."
        ]
    );
    let small = fit(&context, Kind::Windows, &asked(Some(1), Some(2), Some(20), None), USUAL, 0);
    assert_eq!(fitted(&small), (2, 4, 40, 0));
    assert_eq!(
        small.notes,
        [
            "The recipe asks for 1 cores, and this PC allows 2, so the machine has 2.",
            "The recipe asks for 2 GiB of RAM, and this PC allows 4, so the machine has 4.",
            "The recipe asks for 20 GiB of disk, and this PC allows 40, so the machine has 40."
        ]
    );
    let out_of_reach = fit(&context, Kind::Windows, &none, Usual { cores: 99, ram_gib: 99, disk_gib: 1 }, 8);
    assert_eq!((fitted(&out_of_reach), out_of_reach.notes.len()), ((8, 24, 40, 0), 0));
    let linux = fit(&context, Kind::Linux, &asked(Some(1), Some(1), Some(1), None), USUAL, 0);
    assert_eq!((fitted(&linux), linux.notes.len()), ((1, 1, 1, 0), 0));
    // 32 GiB, less what the host keeps of it.
    let plain = fit(&context, Kind::Plain, &asked(Some(64), Some(64), None, None), USUAL, 0);
    assert_eq!(fitted(&plain), (8, 24, 64, 0));
    assert_eq!(
        plain.notes,
        [
            "The recipe asks for 64 cores, and this PC allows 8, so the machine has 8.",
            "The recipe asks for 64 GiB of RAM, and this PC allows 24, so the machine has 24."
        ]
    );
}

// Lean Windows.

#[test]
fn lean_files_are_named_as_they_were() {
    assert_eq!(
        (lean::LEAN_FILE, lean::RESULT_FILE, lean::SCRIPT, lean::DATA_ON_DISC),
        ("lean.json", "lean-result.json", "lean.ps1", "lean.json")
    );
}

#[test]
fn lean_json_is_written_and_read_as_it_was() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    assert!(!lean::is_lean(&context, &vm));
    lean::write_lean(&context, &vm, false).unwrap();
    assert!(!vm.folder(&context).join(lean::LEAN_FILE).exists());
    lean::write_lean(&context, &vm, true).unwrap();
    same("lean.json", read(&vm, &context, lean::LEAN_FILE));
    assert!(lean::is_lean(&context, &vm));
    lean::write_lean(&context, &vm, false).unwrap();
    assert!(!vm.folder(&context).join(lean::LEAN_FILE).exists());
    let write = |text: &str| fs::write(vm.folder(&context).join(lean::LEAN_FILE), text).unwrap();
    for (text, lean) in [
        (r#"{"lean": true}"#, true),
        (r#"{"lean": false}"#, false),
        (r#"{"lean": true, "more": 1}"#, false),
        (r#"{}"#, false),
        ("true", false),
    ] {
        write(text);
        assert_eq!(lean::is_lean(&context, &vm), lean, "{text}");
    }
}

#[test]
fn the_list_on_a_lean_disc_is_as_it_was() {
    same("lean-on-disc.json", lean::guest_data() + "\n");
    assert_eq!(lean::APPS.len(), 22);
    assert_eq!(lean::POLICIES.len(), 5);
}

#[test]
fn what_lean_windows_does_is_said_as_it_was() {
    same("lean-what-it-does.txt", lean::what_it_does() + "\n");
    assert_eq!(lean::LEFT_ALONE.len(), 6);
}

#[test]
fn lean_result_json_is_written_and_read_as_it_was() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    assert_eq!(lean::read_outcome(&context, &vm), lean::Outcome::default());
    let outcome = lean::Outcome {
        taken_out: vec!["MSTeams".into(), "Microsoft.BingNews".into()],
        put_back: vec!["Microsoft.Windows.DevHome".into()],
        still_back: Some("in use".into()),
    };
    lean::write_outcome(&context, &vm, &outcome).unwrap();
    same("lean-result.json", read(&vm, &context, lean::RESULT_FILE));
    assert_eq!(lean::read_outcome(&context, &vm), outcome);
    let plain = lean::Outcome { taken_out: vec!["MSTeams".into()], ..lean::Outcome::default() };
    lean::write_outcome(&context, &vm, &plain).unwrap();
    same("lean-result.plain.json", read(&vm, &context, lean::RESULT_FILE));
    fs::write(vm.folder(&context).join(lean::RESULT_FILE), r#"{"put_back": ["x"], "more": 1}"#).unwrap();
    assert_eq!(lean::read_outcome(&context, &vm).put_back, ["x"]);
    fs::write(vm.folder(&context).join(lean::RESULT_FILE), "nothing").unwrap();
    assert_eq!(lean::read_outcome(&context, &vm), lean::Outcome::default());
}

#[test]
fn what_the_guest_says_it_did_is_read_as_it_was() {
    let said = "begin\ndeprovisioned Microsoft.BingNews\ndeprovisioned Evil.App\n  removed Microsoft.BingNews \n\
                removed MSTeams\nkept Clipchamp.Clipchamp x\nremoved  Microsoft.Todos\nend 1\n";
    assert_eq!(lean::names_taken_out(said), ["MSTeams", "Microsoft.BingNews"]);
    assert_eq!(
        lean::names_back("back Microsoft.Windows.DevHome\nback Nope.Nope\n back MSTeams\nchecked\nback\n"),
        ["MSTeams", "Microsoft.Windows.DevHome"]
    );
    assert!(lean::names_taken_out("").is_empty());
    let names = ["Microsoft.ZuneVideo".to_owned(), "Unknown.App".to_owned(), "MSTeams".to_owned()];
    assert_eq!(lean::titles(&names), "Movies & TV, Unknown.App, Teams (free)");
    assert_eq!(lean::titles(&[]), "");
}

#[test]
fn what_lean_windows_did_is_said_as_it_was() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    let says = |outcome: lean::Outcome| {
        lean::write_outcome(&context, &vm, &outcome).unwrap();
        lean::state_text(&context, &vm)
    };
    assert_eq!(lean::state_text(&context, &vm), None);
    assert_eq!(says(lean::Outcome::default()), None);
    let taken = vec!["Microsoft.BingNews".to_owned(), "MSTeams".to_owned()];
    assert_eq!(
        says(lean::Outcome { taken_out: taken.clone(), ..lean::Outcome::default() }).unwrap(),
        "Took out News, Teams (free), which this Windows had."
    );
    assert_eq!(
        says(lean::Outcome {
            put_back: vec!["Microsoft.Windows.DevHome".into()],
            ..lean::Outcome::default()
        })
        .unwrap(),
        "Windows had put back Dev Home, and a start took it out again."
    );
    assert_eq!(
        says(lean::Outcome {
            taken_out: taken.clone(),
            put_back: vec!["Microsoft.Windows.DevHome".into(), "Microsoft.BingNews".into()],
            still_back: None,
        })
        .unwrap(),
        "Took out News, Teams (free), which this Windows had. Windows had put back Dev Home, News, and a start \
         took them out again."
    );
    assert_eq!(
        says(lean::Outcome {
            taken_out: vec![],
            put_back: vec!["Microsoft.Windows.DevHome".into()],
            still_back: Some("in use".into()),
        })
        .unwrap(),
        "Windows had put back Dev Home, and it could not be taken out again: in use"
    );
    assert_eq!(
        says(lean::Outcome {
            taken_out: taken,
            put_back: vec!["Microsoft.Windows.DevHome".into(), "Clipchamp.Clipchamp".into()],
            still_back: Some("in use".into()),
        })
        .unwrap(),
        "Took out News, Teams (free), which this Windows had. Windows had put back Dev Home, Clipchamp, and they \
         could not be taken out again: in use"
    );
    assert_eq!(
        says(lean::Outcome { still_back: Some("why".into()), ..lean::Outcome::default() }),
        None,
        "a reason with nothing put back says nothing"
    );
}

#[test]
fn an_imported_lean_machine_is_remade_as_it_was() {
    let scratch = Scratch::new();
    let folder = scratch.path("imported");
    fs::create_dir_all(&folder).unwrap();
    let choice = folder.join(lean::LEAN_FILE);
    let result = folder.join(lean::RESULT_FILE);
    fs::write(&choice, "{\"lean\":true}").unwrap();
    fs::write(
        &result,
        r#"{"taken_out": ["Evil.App", "MSTeams", "Microsoft.BingNews"], "put_back": ["Microsoft.Windows.DevHome"], "still_back": "said"}"#,
    )
    .unwrap();
    lean::remake_imported(&folder).unwrap();
    same("lean.json", fs::read_to_string(&choice).unwrap());
    same("lean-result.imported.json", fs::read_to_string(&result).unwrap());
    fs::write(&choice, r#"{"lean": false, "x": 1}"#).unwrap();
    fs::write(&result, r#"{"taken_out": ["Evil.App"]}"#).unwrap();
    lean::remake_imported(&folder).unwrap();
    assert!(!choice.exists() && !result.exists());
    fs::write(&choice, r#"{"lean": false}"#).unwrap();
    fs::write(&result, "not json").unwrap();
    lean::remake_imported(&folder).unwrap();
    assert!(!choice.exists() && !result.exists());
    lean::remake_imported(&folder).unwrap();
    assert!(!choice.exists() && !result.exists());
}

#[test]
fn lean_runs_from_the_disc_by_the_names_the_answer_file_gives() {
    let template = include_str!("../../../guest/windows/autounattend.xml");
    let with_step = lean::with_specialize_step(template).unwrap();
    let step = with_step.lines().find(|line| line.contains(lean::SCRIPT)).unwrap();
    assert!(step.contains(&format!(":\\{} -Data %i:\\{}", lean::SCRIPT, lean::DATA_ON_DISC)), "{step}");
    assert_eq!(
        lean::with_specialize_step("<unattend/>").unwrap_err().to_string(),
        "The answer file has no place for lean Windows' step, so nothing was installed."
    );
}
