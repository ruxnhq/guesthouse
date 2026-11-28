//! Where recipes are found: the ones that come with the app, and the user's own in a folder of the
//! app's config. A file that can't be read or fails its checks is listed as a problem with its name,
//! and never stops the others. Where the two folders are, and which recipes a kind of machine is
//! offered, is the host's.

use super::format::Recipe;
use crate::failure::{Failure, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const EXTENSION: &str = "toml";
/// A file bigger than any recipe is, so a stray big file isn't read to find that out.
const FILE_LONGEST: u64 = 128 * 1024;
const MOST_FILES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Comes with the app.
    App,
    /// The user's own, from their config folder.
    Yours,
}

impl Source {
    fn key_prefix(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Yours => "yours",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `app/games` or `yours/games`: where it's from and the file's name, which tells two of one name apart.
    pub key: String,
    pub source: Source,
    pub file: PathBuf,
    pub recipe: Recipe,
}

#[derive(Debug, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Files that were not recipes, each said with its name.
    pub problems: Vec<String>,
}

fn read_folder(folder: &Path, source: Source, into: &mut Listing) {
    let Ok(read) = fs::read_dir(folder) else { return };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == EXTENSION))
        .collect();
    files.sort();
    for file in files.into_iter().take(MOST_FILES) {
        let stem = file.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let key = format!("{}/{stem}", source.key_prefix());
        match read_recipe(&file) {
            Ok(recipe) => into.entries.push(Entry { key, source, file, recipe }),
            Err(failure) => into.problems.push(format!("{key}.toml: {failure}")),
        }
    }
}

fn read_recipe(file: &Path) -> Result<Recipe> {
    let found = fs::metadata(file)?;
    if !found.is_file() || found.len() > FILE_LONGEST {
        return Err(Failure::new("It is not a recipe file."));
    }
    let text = fs::read_to_string(file).map_err(|_| Failure::new("It is not text."))?;
    Recipe::parse(&text)
}

/// Every recipe there is, the app's first: `shipped` holds the ones that come with the app, and
/// `yours` the user's own.
pub fn list(shipped: &Path, yours: &Path) -> Listing {
    let mut listing = Listing::default();
    read_folder(shipped, Source::App, &mut listing);
    read_folder(yours, Source::Yours, &mut listing);
    listing
}

/// The recipe a person named, by its key, its file's name or its name in any case, from the ones a
/// machine can have. Two that fit are told apart by their keys.
pub fn find(mut listing: Listing, wanted: &str) -> Result<Entry> {
    let wanted = wanted.trim().to_lowercase();
    let fits = |entry: &Entry| {
        let stem = entry.key.split_once('/').map_or("", |(_, stem)| stem);
        [entry.key.to_lowercase(), stem.to_lowercase(), entry.recipe.name.to_lowercase()].contains(&wanted)
    };
    let exact = listing.entries.iter().position(|entry| entry.key.to_lowercase() == wanted);
    let matching: Vec<usize> =
        (0..listing.entries.len()).filter(|index| fits(&listing.entries[*index])).collect();
    match (exact, matching.as_slice()) {
        (Some(index), _) | (None, &[index]) => Ok(listing.entries.swap_remove(index)),
        (None, []) => {
            let keys: Vec<&str> = listing.entries.iter().map(|entry| entry.key.as_str()).collect();
            let have = if keys.is_empty() {
                "There are none for this machine.".to_owned()
            } else {
                format!("There are {}.", keys.join(", "))
            };
            Err(Failure(format!("There is no recipe called {wanted} for this machine. {have}")))
        }
        (None, many) => {
            let keys: Vec<&str> = many.iter().map(|index| listing.entries[*index].key.as_str()).collect();
            Err(Failure(format!(
                "More than one recipe is called {wanted}: {}. Name one of them.",
                keys.join(", ")
            )))
        }
    }
}
