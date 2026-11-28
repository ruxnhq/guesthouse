//! The Linux host's rules for a Windows guest's names and paths: [`file_names`] for a file that goes
//! into the guest or comes out of it, and [`scan_exclusions`] for the folders Defender may leave out.

pub mod file_names;
pub mod scan_exclusions;

#[cfg(test)]
mod tests;
