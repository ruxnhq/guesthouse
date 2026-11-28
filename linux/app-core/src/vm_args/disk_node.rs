//! A machine's qcow2 disk as one `-blockdev` node, with the files it's built on given to QEMU as its
//! backing nodes, nearest first, and none after the last. QEMU then never takes a backing file's name
//! from the disk's header, which it may have written itself: it opens the files the machine's settings
//! name, which a start has checked the header against (see [`crate::disk_image::backing`]).

use super::VmPlan;
use crate::disk_image::backing::{BackingFile, DISK_FORMAT};
use serde_json::{Value, json};

/// A raw file is all data, so it has no backing of its own to name.
const RAW: &str = "raw";

/// How QEMU caches a disk: see [`VmPlan::disk_cache`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cache {
    /// The guest's flushes reach the host's disk.
    Writeback,
    /// They are dropped.
    Unsafe,
}

/// What a disk's node is given besides its file and its backing files.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskOptions {
    pub cache: Option<Cache>,
    /// Space the guest frees is given back on the host.
    pub discard: bool,
}

fn file_node(path: &str, discard: bool) -> Value {
    let mut node = json!({"driver": "file", "filename": path});
    if discard {
        node["discard"] = json!("unmap");
    }
    node
}

/// The backing files as nested nodes, nearest first, read only, and the last with none.
fn backing_of(chain: &[BackingFile]) -> Value {
    chain.iter().rev().fold(Value::Null, |below, file| {
        let mut node = json!({"driver": file.format, "file": file_node(&file.path, false)});
        if file.format != RAW {
            node["backing"] = below;
        }
        node
    })
}

/// The `-blockdev` word for a qcow2 disk at `path`, as the node `name`, built on `chain`.
pub fn disk_node(name: &str, path: &str, chain: &[BackingFile], options: DiskOptions) -> String {
    let mut node = json!({
        "driver": DISK_FORMAT,
        "node-name": name,
        "file": file_node(path, options.discard),
        "backing": backing_of(chain),
    });
    if options.discard {
        node["discard"] = json!("unmap");
    }
    if let Some(cache) = options.cache {
        node["cache"] = json!({"direct": false, "no-flush": cache == Cache::Unsafe});
    }
    node.to_string()
}

/// The words that open a running QEMU's disk by its path, in a `-blockdev` word, however the path
/// has to be written in JSON.
pub fn names_disk_file(word: &[u8], path: &str) -> bool {
    let wanted = format!("\"filename\":{}", Value::String(path.to_owned()));
    word.windows(wanted.len()).any(|part| part == wanted.as_bytes())
}

impl VmPlan {
    /// The files QEMU reads the machine's disk through, nearest first, as the settings name them: a
    /// throwaway run's overlay is on the machine's own disk, which is on its backing files.
    pub fn disk_chain(&self) -> Vec<BackingFile> {
        let own = self
            .throwaway
            .iter()
            .map(|disk| BackingFile { path: disk.clone(), format: DISK_FORMAT.to_owned() });
        own.chain(self.backing_files.iter().cloned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, format: &str) -> BackingFile {
        BackingFile { path: path.into(), format: format.into() }
    }

    #[test]
    fn a_disk_on_its_own_has_no_backing_file_whatever_its_header_says() {
        let word = disk_node("hd0", "/m/a,b \"x\"/disk.qcow2", &[], DiskOptions::default());
        let node: Value = serde_json::from_str(&word).unwrap();
        assert_eq!(node["backing"], Value::Null);
        assert!(node.as_object().unwrap().contains_key("backing"));
        assert_eq!(node["file"]["filename"], "/m/a,b \"x\"/disk.qcow2");
        assert!(names_disk_file(word.as_bytes(), "/m/a,b \"x\"/disk.qcow2"));
        assert!(!names_disk_file(word.as_bytes(), "/m/a,b"));
    }

    #[test]
    fn each_backing_file_is_a_node_and_the_last_has_none() {
        let chain = [file("/b/middle.qcow2", "qcow2"), file("/b/base.qcow2", "qcow2")];
        let options = DiskOptions { cache: Some(Cache::Writeback), discard: true };
        let node: Value = serde_json::from_str(&disk_node("hd0", "/m/disk.qcow2", &chain, options)).unwrap();
        assert_eq!(node["backing"]["file"]["filename"], "/b/middle.qcow2");
        assert_eq!(node["backing"]["backing"]["file"]["filename"], "/b/base.qcow2");
        assert_eq!(node["backing"]["backing"]["backing"], Value::Null);
        assert_eq!(
            (node["discard"].clone(), node["file"]["discard"].clone()),
            (json!("unmap"), json!("unmap"))
        );
        assert_eq!(node["cache"], json!({"direct": false, "no-flush": false}));
        let raw: Value = serde_json::from_str(&disk_node(
            "d",
            "/d",
            &[file("/b/base.img", "raw")],
            DiskOptions::default(),
        ))
        .unwrap();
        assert_eq!(
            raw["backing"],
            json!({"driver": "raw", "file": {"driver": "file", "filename": "/b/base.img"}})
        );
    }
}
