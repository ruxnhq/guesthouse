use super::*;

const GOOD: &str = r#"
name = "Games"
description = "Windows with room for games."
guest = "windows"

[settings]
cores = 8
ram_gib = 16
disk_gib = 200
vram_gib = 0

[steps]
packages = ["Valve.Steam", "7zip.7zip"]
script = '''
Write-Output "hello"
'''
"#;

fn refused(text: &str) -> String {
    Recipe::parse(text).unwrap_err().0
}

fn windows_with(steps: &str) -> String {
    format!("name = \"A\"\ndescription = \"B\"\nguest = \"windows\"\n[steps]\n{steps}\n")
}

#[test]
fn a_whole_recipe_is_read() {
    let recipe = Recipe::parse(GOOD).unwrap();
    assert_eq!(recipe.name, "Games");
    assert_eq!(recipe.guest, Guest::Windows);
    assert_eq!(
        recipe.settings,
        SettingsAsked { cores: Some(8), ram_gib: Some(16), disk_gib: Some(200), vram_gib: Some(0) }
    );
    assert_eq!(recipe.steps.packages, ["Valve.Steam", "7zip.7zip"]);
    assert_eq!(recipe.steps.script.as_deref(), Some("Write-Output \"hello\"\n"));
}

#[test]
fn a_recipe_may_ask_for_nothing_at_all() {
    let recipe =
        Recipe::parse("name = \"Clean\"\ndescription = \"Nothing extra.\"\nguest = \"ubuntu\"\n").unwrap();
    assert!(recipe.settings.is_empty() && recipe.steps.is_empty());
    assert!(recipe.step_names().is_empty());
}

#[test]
fn steps_are_named_in_the_order_they_run() {
    let recipe = Recipe::parse(GOOD).unwrap();
    assert_eq!(
        recipe.step_names(),
        ["Installing Valve.Steam", "Installing 7zip.7zip", "Running the recipe's PowerShell script"]
    );
}

#[test]
fn a_word_the_format_does_not_have_is_refused_and_not_ignored() {
    assert!(refused(&format!("{GOOD}\nrun_as = \"system\"\n")).contains("run_as"));
    assert!(refused(&GOOD.replace("[settings]", "[settings]\nsound = true")).contains("sound"));
    assert!(refused(&GOOD.replace("windows\"", "macos\"")).contains("macos"));
}

#[test]
fn a_mistake_in_the_file_says_which_line() {
    let said = refused("name = \"A\"\ndescription = \"B\"\nguest = windows\n");
    assert!(said.contains("line 3"), "{said}");
}

#[test]
fn a_package_name_can_only_be_a_name() {
    for bad in [
        "--source", "-h", "a;b", "a b", "a&b", "$(x)", "a\"b", "a'b", "a|b", "a\nb", "", "x", "../x", "a/b",
        "`x`", "a%b",
    ] {
        let said = refused(&windows_with(&format!("packages = [{bad:?}]")));
        assert!(said.contains("is not a name"), "{bad:?}: {said}");
    }
    for bad in ["Firefox", "-y", "a;b", "a b", "pkg:amd64", "a=1.0", "a*"] {
        let text = "name = \"A\"\ndescription = \"B\"\nguest = \"ubuntu\"\n[steps]\npackages = [".to_owned()
            + &format!("{bad:?}]\n");
        let said = refused(&text);
        assert!(said.contains("is not a name"), "{bad:?}: {said}");
    }
}

#[test]
fn names_that_packages_really_have_are_taken() {
    assert!(
        Recipe::parse(&windows_with(
            "packages = [\"Microsoft.VisualStudioCode\", \"Notepad++.Notepad++\", \"Git.Git\"]"
        ))
        .is_ok()
    );
    let ubuntu = "name = \"A\"\ndescription = \"B\"\nguest = \"ubuntu\"\n[steps]\npackages = [\"build-essential\", \"libstdc++-13-dev\", \"python3.12\", \"g++\"]\n";
    // g++ is a real package name, and a plus is allowed.
    assert!(Recipe::parse(ubuntu).is_ok());
}

#[test]
fn what_is_too_long_or_too_many_is_refused() {
    let many: Vec<String> = (0..=PACKAGES_MOST).map(|number| format!("\"Pkg.N{number}\"")).collect();
    assert!(refused(&windows_with(&format!("packages = [{}]", many.join(",")))).contains("at most"));
    let long = "x".repeat(SCRIPT_LONGEST + 1);
    assert!(refused(&windows_with(&format!("script = '{long}'"))).contains("longer"));
    let name = "n".repeat(NAME_LONGEST + 1);
    assert!(refused(&GOOD.replace("Games", &name)).contains("name is longer"));
    assert!(refused(&"#".repeat(70_000)).contains("at most"));
}

#[test]
fn a_name_and_a_description_are_one_line_of_plain_text() {
    assert!(refused(&GOOD.replace("name = \"Games\"", "name = \"A\\nB\"")).contains("one line"));
    assert!(refused(&GOOD.replace("Windows with room for games.", "x\\u001b[2Jy")).contains("one line"));
    assert!(refused(&GOOD.replace("name = \"Games\"", "name = \"  \"")).contains("empty"));
}

#[test]
fn a_setting_of_nothing_is_refused_except_for_the_video_memory_cap() {
    assert!(refused(&GOOD.replace("cores = 8", "cores = 0")).contains("cores must be more than 0"));
    assert!(refused(&GOOD.replace("ram_gib = 16", "ram_gib = 0")).contains("ram_gib"));
    assert!(Recipe::parse(GOOD).is_ok(), "vram_gib = 0 is no cap");
    assert!(refused(&GOOD.replace("cores = 8", "cores = -1")).contains("on line 7"));
}

#[test]
fn only_a_windows_recipe_has_a_video_memory_cap() {
    let ubuntu = "name = \"A\"\ndescription = \"B\"\nguest = \"ubuntu\"\n[settings]\nvram_gib = 4\n";
    assert!(refused(ubuntu).contains("only Windows"));
}

#[test]
fn an_empty_script_is_refused() {
    assert!(refused(&windows_with("script = '   '")).contains("script is empty"));
    assert!(refused(&windows_with("script = \"a\\u0000b\"")).contains("script"));
}

#[test]
fn the_guests_that_have_recipes_are_the_ones_with_a_way_in() {
    assert_eq!(Guest::of_kind(Kind::Windows), Some(Guest::Windows));
    assert_eq!(Guest::of_kind(Kind::Linux), Some(Guest::Ubuntu));
    assert_eq!(Guest::of_kind(Kind::Plain), None);
}

#[test]
fn marks_that_hide_or_reverse_text_are_refused_where_a_person_reads_one_line() {
    for mark in ["\\u202e", "\\u200b", "\\ufeff", "\\u2066"] {
        assert!(refused(&GOOD.replace("Games", &format!("Ga{mark}mes"))).contains("one line"), "{mark}");
    }
}

#[test]
fn lean_is_for_windows_and_a_recipe_without_it_is_kept_as_before() {
    let lean =
        Recipe::parse("name = \"S\"\ndescription = \"D.\"\nguest = \"windows\"\nlean = true\n").unwrap();
    assert!(lean.lean);
    assert!(serde_json::to_string(&lean).unwrap().contains("\"lean\":true"));
    let usual = Recipe::parse("name = \"S\"\ndescription = \"D.\"\nguest = \"windows\"\n").unwrap();
    assert!(!usual.lean);
    assert!(!serde_json::to_string(&usual).unwrap().contains("lean"), "an older build still reads its copy");
    let said =
        Recipe::parse("name = \"S\"\ndescription = \"D.\"\nguest = \"ubuntu\"\nlean = true\n").unwrap_err();
    assert!(said.0.contains("lean is for a Windows recipe"), "{}", said.0);
}
