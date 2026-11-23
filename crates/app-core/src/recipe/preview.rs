//! What a recipe says it will do, in words, for the person who is about to say yes. It shows the whole
//! script, as the script is the part that can do anything in the guest.

use super::format::{Guest, Recipe, hidden};
use super::record::{Outcome, read_chosen, read_outcome};
use super::shelf::Source;
use crate::context::Context;
use crate::machine::Machine;
use std::fmt::Write;

const INDENT: &str = "    ";

/// Where a recipe is from, and what that means for what it runs.
pub fn origin_text(source: Source, file: &str) -> String {
    match source {
        Source::App => "It comes with Guesthouse.".to_owned(),
        Source::Yours => format!("It is your own recipe, from {file}. Nobody else has looked at it."),
    }
}

fn settings_text(recipe: &Recipe) -> Option<String> {
    let asked = &recipe.settings;
    let mut parts = Vec::new();
    if let Some(cores) = asked.cores {
        parts.push(format!("{cores} cores"));
    }
    if let Some(ram) = asked.ram_gib {
        parts.push(format!("{ram} GiB of RAM"));
    }
    if let Some(disk) = asked.disk_gib {
        parts.push(format!("a {disk} GiB disk"));
    }
    match asked.vram_gib {
        Some(0) => parts.push("no cap on video memory".to_owned()),
        Some(vram) => parts.push(format!("video memory capped at {vram} GiB")),
        None => {}
    }
    (!parts.is_empty()).then(|| format!("It sets {}.", parts.join(", ")))
}

/// The script as it will run, with every character that would draw nothing shown as its code, so that
/// what a terminal or a dialog shows is all there is. Line breaks and tabs stay.
fn visible(text: &str) -> String {
    text.chars()
        .map(|c| {
            if hidden(c) && c != '\n' && c != '\t' { format!("\\u{{{:x}}}", c as u32) } else { c.to_string() }
        })
        .collect()
}

/// The recipe as a block of text: what it is, where it's from, what it sets, and each step that runs.
pub fn what_it_will_run(recipe: &Recipe, source: Source, file: &str) -> String {
    let mut text = format!("{}: {}\n{}\n", recipe.name, recipe.description, origin_text(source, file));
    text.push_str(&settings_text(recipe).unwrap_or_default());
    if recipe.steps.is_empty() {
        text.push_str("\nIt runs nothing in the machine.");
        return text.trim().to_owned();
    }
    let who = match recipe.guest {
        Guest::Windows => "as the account's administrator",
        Guest::Ubuntu => "as root",
    };
    let _ =
        write!(text, "\nOnce {} is installed and up, it runs in the machine, {who}:", recipe.guest.name());
    for name in &recipe.steps.packages {
        let _ = write!(text, "\n- Install the {} {name}", recipe.guest.package_name());
    }
    if let Some(script) = &recipe.steps.script {
        let _ = write!(text, "\n- Run this {}:\n", recipe.guest.script_name());
        for line in visible(script.trim_end()).lines() {
            let _ = writeln!(text, "{INDENT}{line}");
        }
    }
    text.trim().to_owned()
}

/// How the steps of a machine's recipe went, and what a throwaway does to them.
fn outcome_text(context: &Context, machine: &Machine, recipe: &Recipe) -> String {
    let went = if recipe.steps.is_empty() {
        "It has no steps to run.".to_owned()
    } else {
        match read_outcome(context, machine) {
            Outcome::Pending => "Its steps have not run yet.".to_owned(),
            Outcome::Done => "Its steps ran.".to_owned(),
            Outcome::Failed { step, why } => format!("Its steps stopped at \"{step}\": {why}"),
        }
    };
    let throwaway = if machine.throwaway && !recipe.steps.is_empty() {
        " On a throwaway, what they install is gone when it shuts down."
    } else {
        ""
    };
    format!("{went}{throwaway}")
}

/// Where a machine's recipe is from and how its steps went, for a row that has the recipe's name as its
/// title already. Nothing for a machine made with none.
pub fn origin_and_outcome(context: &Context, machine: &Machine) -> Option<String> {
    let chosen = read_chosen(context, machine)?;
    let from = match chosen.source {
        Source::App => "It comes with Guesthouse.",
        Source::Yours => "It is your own recipe.",
    };
    Some(format!("{from} {}", outcome_text(context, machine, &chosen.recipe)))
}

/// A machine's recipe by name, and how its steps went, in a sentence or two, or nothing for a machine made
/// with none.
pub fn summary(context: &Context, machine: &Machine) -> Option<String> {
    let chosen = read_chosen(context, machine)?;
    let name = &chosen.recipe.name;
    let from = match chosen.source {
        Source::App => "from Guesthouse",
        Source::Yours => "your own",
    };
    Some(format!("{name} ({from}). {}", outcome_text(context, machine, &chosen.recipe)))
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
