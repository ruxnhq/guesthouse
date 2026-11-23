use super::*;
use crate::testing::{Scratch, scratch_context};

#[test]
fn only_names_on_the_list_are_believed_each_once() {
    let said = "begin\ndeprovisioned Microsoft.BingNews\ndeprovisioned Evil.App\nremoved Microsoft.BingNews\n\
                removed MSTeams\nkept Clipchamp.Clipchamp x\nend 1\n";
    assert_eq!(names_taken_out(said), ["MSTeams", "Microsoft.BingNews"]);
    assert_eq!(
        names_back("back Microsoft.Windows.DevHome\nback Nope.Nope\nchecked\n"),
        ["Microsoft.Windows.DevHome"]
    );
}

#[test]
fn the_machine_says_what_was_taken_out_and_what_came_back() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = Machine::new("box", "Box", "/d", "");
    fs::create_dir_all(vm.folder(&context)).unwrap();
    assert_eq!(state_text(&context, &vm), None);
    let mut outcome =
        Outcome { taken_out: vec!["Microsoft.BingNews".into(), "MSTeams".into()], ..Outcome::default() };
    write_outcome(&context, &vm, &outcome).unwrap();
    assert_eq!(state_text(&context, &vm).unwrap(), "Took out News, Teams (free), which this Windows had.");
    outcome.put_back = vec!["Microsoft.Windows.DevHome".into()];
    write_outcome(&context, &vm, &outcome).unwrap();
    assert!(
        state_text(&context, &vm)
            .unwrap()
            .ends_with("Windows had put back Dev Home, and a start took it out again.")
    );
    outcome.still_back = Some("in use".into());
    write_outcome(&context, &vm, &outcome).unwrap();
    assert!(state_text(&context, &vm).unwrap().ends_with("could not be taken out again: in use"));
}
