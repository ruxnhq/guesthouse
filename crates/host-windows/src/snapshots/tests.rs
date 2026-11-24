use super::*;
use serde_json::json;
use std::cell::Cell;

/// Disks as files of JSON: a disk's own writes, and its parent by path with the parent's generation
/// when it was made. A merge changes the parent's generation, so a disk made on it before no longer
/// reads, as a real one wouldn't. A disk whose parent is gone doesn't read either.
#[derive(Default)]
struct Fake {
    fail_next: Cell<bool>,
}

fn read_file(path: &Path) -> Result<serde_json::Value, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(serde_json::from_str(&text).unwrap())
}

/// What a disk reads as: its parents' writes, then its own.
fn read(path: &Path) -> Result<Vec<String>, String> {
    let disk = read_file(path)?;
    let mut data = match disk["parent"].as_str() {
        Some(parent) => {
            let parent_disk = read_file(Path::new(parent))?;
            if parent_disk["gen"] != disk["parent_gen"] {
                return Err(format!("{} has changed under {}", parent, path.display()));
            }
            read(Path::new(parent))?
        }
        None => Vec::new(),
    };
    data.extend(disk["data"].as_array().unwrap().iter().map(|item| item.as_str().unwrap().to_owned()));
    Ok(data)
}

/// The guest writes `what` to the disk.
fn write(path: &Path, what: &str) {
    let mut disk = read_file(path).unwrap();
    disk["data"].as_array_mut().unwrap().push(json!(what));
    fs::write(path, disk.to_string()).unwrap();
}

impl Disks for Fake {
    fn differencing(&self, child: &Path, parent: &Path) -> Result<(), String> {
        if self.fail_next.replace(false) {
            return Err("the disk couldn't be made".into());
        }
        assert!(!child.exists(), "{} is made only once", child.display());
        read(parent)?;
        let generation = read_file(parent)?["gen"].clone();
        let disk =
            json!({"parent": parent.to_str().unwrap(), "parent_gen": generation, "gen": 0, "data": []});
        fs::write(child, disk.to_string()).map_err(|error| error.to_string())
    }

    fn merge_into_parent(&self, child: &Path) -> Result<(), String> {
        read(child)?;
        let disk = read_file(child)?;
        let parent = PathBuf::from(disk["parent"].as_str().ok_or("it has no parent")?);
        let mut parent_disk = read_file(&parent)?;
        for item in disk["data"].as_array().unwrap() {
            parent_disk["data"].as_array_mut().unwrap().push(item.clone());
        }
        parent_disk["gen"] = json!(parent_disk["gen"].as_u64().unwrap() + 1);
        fs::write(&parent, parent_disk.to_string()).map_err(|error| error.to_string())
    }
}

struct Machine {
    root: PathBuf,
    disk: PathBuf,
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn machine(test: &str) -> Machine {
    let root = std::env::temp_dir().join(format!("gh-snapshots-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let disk = root.join("win.vhdx");
    fs::write(&disk, json!({"parent": null, "gen": 0, "data": ["installed"]}).to_string()).unwrap();
    fs::write(state_file(&disk), "tpm 1").unwrap();
    Record { installed: true, ..Record::default() }.save(&disk).unwrap();
    Machine { root, disk }
}

#[test]
fn going_back_gives_the_disk_its_tpm_and_its_record_as_they_were() {
    let m = machine("back");
    let id = take(&m.disk, "before", 100, &Fake::default()).unwrap();
    write(&m.disk, "after");
    fs::write(state_file(&m.disk), "tpm 2").unwrap();
    Record { installed: true, screen: Some("later".into()), ..Record::default() }.save(&m.disk).unwrap();
    assert_eq!(read(&m.disk).unwrap(), ["installed", "after"]);
    assert!(made_on_another(&m.disk).unwrap());

    revert(&m.disk, id, &Fake::default()).unwrap();
    assert_eq!(read(&m.disk).unwrap(), ["installed"]);
    assert_eq!(fs::read_to_string(state_file(&m.disk)).unwrap(), "tpm 1");
    assert_eq!(Record::load(&m.disk).unwrap().screen, None);
    assert_eq!(list(&m.disk).unwrap(), [Snapshot { id, label: "before".into(), taken: 100, current: true }]);
}

#[test]
fn deleting_every_snapshot_leaves_one_disk_as_it_is() {
    let m = machine("delete-all");
    let fake = Fake::default();
    let first = take(&m.disk, "one", 1, &fake).unwrap();
    write(&m.disk, "b");
    let second = take(&m.disk, "two", 2, &fake).unwrap();
    write(&m.disk, "c");
    assert_eq!(chain(&m.disk).unwrap(), [snapshot_disk(&m.disk, second), snapshot_disk(&m.disk, first)]);

    delete(&m.disk, first, &fake).unwrap();
    assert!(snapshot_disk(&m.disk, first).exists(), "kept while the second is made on it");
    assert_eq!(list(&m.disk).unwrap().len(), 1);
    delete(&m.disk, second, &fake).unwrap();
    assert_eq!(read(&m.disk).unwrap(), ["installed", "b", "c"]);
    assert!(!made_on_another(&m.disk).unwrap(), "a disk of its own again, which can grow");
    let left: Vec<String> = fs::read_dir(folder(&m.disk))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["index.json"]);
}

#[test]
fn a_disk_that_couldnt_be_made_leaves_the_machine_as_it_was() {
    let m = machine("fail");
    let fake = Fake::default();
    fake.fail_next.set(true);
    assert!(take(&m.disk, "x", 1, &fake).is_err());
    assert_eq!(read(&m.disk).unwrap(), ["installed"]);
    assert!(list(&m.disk).unwrap().is_empty());
    assert!(!made_on_another(&m.disk).unwrap());
    let id = take(&m.disk, "y", 2, &fake).unwrap();
    fake.fail_next.set(true);
    write(&m.disk, "kept");
    assert!(revert(&m.disk, id, &fake).is_err());
    assert_eq!(read(&m.disk).unwrap(), ["installed", "kept"], "a failed revert keeps what was there");
}

#[test]
fn a_take_cut_short_after_the_disk_moved_is_undone() {
    let m = machine("crash-take");
    let mut index = load(&m.disk).unwrap();
    index.taking = Some(Entry { id: 1, label: "x".into(), taken: 1, parent: None, hidden: false });
    index.next_id = 2;
    save(&m.disk, &index).unwrap();
    fs::copy(state_file(&m.disk), state_file(&snapshot_disk(&m.disk, 1))).unwrap();
    fs::rename(&m.disk, snapshot_disk(&m.disk, 1)).unwrap();
    repair(&m.disk).unwrap();
    assert_eq!(read(&m.disk).unwrap(), ["installed"]);
    assert!(!state_file(&snapshot_disk(&m.disk, 1)).exists());
    assert!(list(&m.disk).unwrap().is_empty());
    assert_eq!(load(&m.disk).unwrap().taking, None);
}

#[test]
fn a_collapse_cut_short_before_the_index_was_written_is_finished() {
    let m = machine("crash-collapse");
    let fake = Fake::default();
    let id = take(&m.disk, "x", 1, &fake).unwrap();
    write(&m.disk, "b");
    let mut index = load(&m.disk).unwrap();
    index.snapshots[0].hidden = true;
    save(&m.disk, &index).unwrap();
    fake.merge_into_parent(&m.disk).unwrap();
    fs::rename(snapshot_disk(&m.disk, id), &m.disk).unwrap();
    repair(&m.disk).unwrap();
    assert!(!made_on_another(&m.disk).unwrap());
    assert_eq!(read(&m.disk).unwrap(), ["installed", "b"]);
}

/// Takes, goes back, deletes and writes in a long run, checking after each step that the machine's
/// disk and every snapshot read as a model of them says.
#[test]
fn every_disk_reads_as_it_should_through_any_run_of_steps() {
    let m = machine("run");
    let fake = Fake::default();
    let mut model: Vec<(u64, Vec<String>)> = Vec::new();
    let mut now: Vec<String> = vec!["installed".into()];
    let mut seed: u64 = 7;
    let mut next = |below: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % below
    };
    for step in 0..300u64 {
        match next(5) {
            0 if model.len() < MOST => {
                let id = take(&m.disk, &format!("s{step}"), step, &fake).unwrap();
                model.push((id, now.clone()));
            }
            1 if !model.is_empty() => {
                let (id, data) = model[next(model.len() as u64) as usize].clone();
                revert(&m.disk, id, &fake).unwrap();
                now = data;
            }
            2 if !model.is_empty() => {
                let (id, _) = model.remove(next(model.len() as u64) as usize);
                delete(&m.disk, id, &fake).unwrap();
            }
            _ => {
                let what = format!("w{step}");
                write(&m.disk, &what);
                now.push(what);
            }
        }
        assert_eq!(read(&m.disk).unwrap(), now, "the machine's disk after step {step}");
        for (id, data) in &model {
            assert_eq!(&read(&snapshot_disk(&m.disk, *id)).unwrap(), data, "snapshot {id} after step {step}");
        }
        let shown: Vec<u64> = list(&m.disk).unwrap().iter().map(|snapshot| snapshot.id).collect();
        assert_eq!(shown, model.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    }
    for (id, _) in std::mem::take(&mut model) {
        delete(&m.disk, id, &fake).unwrap();
    }
    assert_eq!(read(&m.disk).unwrap(), now);
    assert!(!made_on_another(&m.disk).unwrap(), "every snapshot deleted, the disk stands alone");
}

/// The same steps on real disk files through virtdisk, with nothing attached, which needs no
/// elevation. It proves the calls and the order of the files, not what the guest wrote.
#[cfg(windows)]
#[test]
fn real_disks_take_go_back_and_delete() {
    use crate::windows::RealDisks;
    use guesthouse_hcs::disk::{Kind, VirtualDisk, create_vhdx};
    let root = std::env::temp_dir().join(format!("gh-real-snapshots-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let disk = root.join("win.vhdx");
    create_vhdx(&disk, 1 << 30).unwrap();
    fs::write(state_file(&disk), "tpm").unwrap();
    let opens =
        |path: &Path| VirtualDisk::open(path, Kind::Vhdx).map(drop).map_err(|error| error.to_string());

    let first = take(&disk, "one", 1, &RealDisks).unwrap();
    let second = take(&disk, "two", 2, &RealDisks).unwrap();
    opens(&disk).unwrap();
    revert(&disk, first, &RealDisks).unwrap();
    opens(&disk).unwrap();
    opens(&snapshot_disk(&disk, second)).unwrap();
    delete(&disk, first, &RealDisks).unwrap();
    opens(&snapshot_disk(&disk, second)).expect("still made on the first, which is kept hidden");
    delete(&disk, second, &RealDisks).unwrap();
    opens(&disk).expect("the disk, merged into the first, opens alone");
    assert!(!made_on_another(&disk).unwrap());
    fs::remove_dir_all(&root).unwrap();
}
