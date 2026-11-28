//! The app's own files, which every report has whatever machine it's about: the log, what setup
//! found, and what the app remembers.

use super::Collection;
use crate::context::Context;
use crate::setup::amd::amd_setup_file;

pub fn collect(context: &Context, found: &mut Collection) {
    let paths = &context.paths;
    found.add_file("app/guesthouse.log", &paths.log_file());
    found.add_file_if_there("app/app.json", &paths.prefs_file());
    found.add_file_if_there("setup/setup.json", &paths.setup_file());
    found.add_file_if_there("setup/amd.json", &amd_setup_file(context));
}
