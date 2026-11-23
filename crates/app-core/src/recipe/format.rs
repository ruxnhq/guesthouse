//! A recipe file, read and checked. Everything a recipe can say is here, with the limits that keep a
//! mistake or a hostile file from reaching a guest as something other than plain data.

use crate::failure::{Failure, Result};
use crate::machine::Kind;
use serde::{Deserialize, Serialize};

pub const NAME_LONGEST: usize = 40;
pub const DESCRIPTION_LONGEST: usize = 160;
pub const PACKAGES_MOST: usize = 30;
/// More than a script of this kind is ever, and little enough to travel as one request to a guest.
pub const SCRIPT_LONGEST: usize = 32 * 1024;
const FILE_LONGEST: usize = 64 * 1024;
const PACKAGE_LONGEST: usize = 128;
const PACKAGE_SHORTEST: usize = 2;

/// The kind of guest a recipe is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Guest {
    Windows,
    Ubuntu,
}

impl Guest {
    pub fn kind(self) -> Kind {
        match self {
            Self::Windows => Kind::Windows,
            Self::Ubuntu => Kind::Linux,
        }
    }

    /// The machine kind's recipes, if it has any: a plain VM and a Linux the user installed have no
    /// way in for us.
    pub fn of_kind(kind: Kind) -> Option<Self> {
        match kind {
            Kind::Windows => Some(Self::Windows),
            Kind::Linux => Some(Self::Ubuntu),
            Kind::Plain => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Ubuntu => "Ubuntu",
        }
    }

    /// What `packages` are, and what runs `script`.
    pub fn package_name(self) -> &'static str {
        match self {
            Self::Windows => "winget package",
            Self::Ubuntu => "apt package",
        }
    }

    pub fn script_name(self) -> &'static str {
        match self {
            Self::Windows => "PowerShell script",
            Self::Ubuntu => "shell script",
        }
    }
}

/// The machine settings a recipe may ask for. Each is one the machine already has, and each is still
/// held to this PC's limits when the machine is made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsAsked {
    pub cores: Option<u32>,
    pub ram_gib: Option<u32>,
    pub disk_gib: Option<u32>,
    /// The most video memory it may use, in GiB. Windows machines only, as only they have the cap.
    pub vram_gib: Option<u32>,
}

impl SettingsAsked {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What runs in the guest, once, after its install has finished and it is up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Steps {
    /// winget ids for Windows, apt names for Ubuntu. Each is a step of its own, in order.
    #[serde(default)]
    pub packages: Vec<String>,
    /// PowerShell for Windows, sh for Ubuntu. It runs after the packages, as the guest's administrator.
    #[serde(default)]
    pub script: Option<String>,
}

impl Steps {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.script.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    pub description: String,
    pub guest: Guest,
    #[serde(default)]
    pub settings: SettingsAsked,
    #[serde(default)]
    pub steps: Steps,
    /// Starts the machine as lean Windows, which the user can still turn off before it is made.
    /// Left out of a file, and of a machine's copy, when it is off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lean: bool,
}

/// A character that draws nothing, or moves what is drawn around it: a control, or one of the marks that
/// reverse or hide text. A recipe is read by a person before it is used, so it can't have any.
pub(super) fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// One line of plain text: nothing that could pass for a second line, or draw over a dialog.
fn plain_line(what: &str, text: &str, longest: usize) -> Result<()> {
    if text.trim().is_empty() {
        return Err(Failure(format!("The recipe's {what} is empty.")));
    }
    if text.chars().count() > longest {
        return Err(Failure(format!("The recipe's {what} is longer than {longest} characters.")));
    }
    if text.chars().any(hidden) {
        return Err(Failure(format!("The recipe's {what} must be one line of plain text.")));
    }
    Ok(())
}

/// A name every package manager here takes as a name and nothing else: letters and digits, with dots,
/// plus signs, underscores and hyphens inside, and never a hyphen first, where it would be an option.
fn package_problem(guest: Guest, name: &str) -> Option<String> {
    let first = name.chars().next().unwrap_or(' ');
    let shape = |c: char| match guest {
        Guest::Windows => c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'),
        Guest::Ubuntu => c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '+' | '-'),
    };
    let fine = (PACKAGE_SHORTEST..=PACKAGE_LONGEST).contains(&name.len())
        && first.is_ascii_alphanumeric()
        && name.chars().all(shape);
    (!fine).then(|| {
        let what = guest.package_name();
        let shown: String = name.chars().filter(|c| !c.is_control()).take(PACKAGE_LONGEST).collect();
        format!("\"{shown}\" is not a name {what}s have. A name is letters, digits and . _ + - only.")
    })
}

impl Recipe {
    /// Reads a recipe from its text and checks all of it, so that nothing past here has to.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > FILE_LONGEST {
            return Err(Failure(format!("A recipe is at most {} KiB.", FILE_LONGEST / 1024)));
        }
        let recipe: Self = toml::from_str(text).map_err(|error| Failure(plain_toml_error(text, &error)))?;
        recipe.check()?;
        Ok(recipe)
    }

    /// Checks what a recipe says. A recipe read back from a machine's folder goes through this too.
    pub fn check(&self) -> Result<()> {
        plain_line("name", &self.name, NAME_LONGEST)?;
        plain_line("description", &self.description, DESCRIPTION_LONGEST)?;
        let must_be_positive = [
            ("cores", self.settings.cores),
            ("ram_gib", self.settings.ram_gib),
            ("disk_gib", self.settings.disk_gib),
        ];
        // The video memory cap is the one setting where 0 is an answer: no cap.
        if let Some(name) =
            must_be_positive.iter().find_map(|(name, value)| (*value == Some(0)).then_some(name))
        {
            return Err(Failure(format!("{name} must be more than 0.")));
        }
        if self.settings.vram_gib.is_some() && self.guest != Guest::Windows {
            return Err(Failure::new(
                "vram_gib is for a Windows recipe: only Windows machines have the cap.",
            ));
        }
        if self.lean && self.guest != Guest::Windows {
            return Err(Failure::new("lean is for a Windows recipe."));
        }
        if self.steps.packages.len() > PACKAGES_MOST {
            return Err(Failure(format!("A recipe lists at most {PACKAGES_MOST} packages.")));
        }
        if let Some(problem) = self.steps.packages.iter().find_map(|name| package_problem(self.guest, name)) {
            return Err(Failure(problem));
        }
        if let Some(script) = &self.steps.script {
            if script.len() > SCRIPT_LONGEST {
                return Err(Failure(format!("The script is longer than {} KiB.", SCRIPT_LONGEST / 1024)));
            }
            if script.trim().is_empty() || script.contains('\0') {
                return Err(Failure::new("The script is empty, or has a character no script has."));
            }
        }
        Ok(())
    }

    pub fn kind(&self) -> Kind {
        self.guest.kind()
    }

    /// Each step in the order it runs, as the words a person reads while it does.
    pub fn step_names(&self) -> Vec<String> {
        let packages = self.steps.packages.iter().map(|name| format!("Installing {name}"));
        let script =
            self.steps.script.iter().map(|_| format!("Running the recipe's {}", self.guest.script_name()));
        packages.chain(script).collect()
    }
}

/// The reason, for a person, and the line it is on. TOML's own message also draws the file.
fn plain_toml_error(text: &str, error: &toml::de::Error) -> String {
    let said = error.message().trim();
    match error.span() {
        Some(span) => {
            let before = &text.as_bytes()[..span.start.min(text.len())];
            let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
            format!("The recipe could not be read, on line {line}: {said}")
        }
        None => format!("The recipe could not be read: {said}"),
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
