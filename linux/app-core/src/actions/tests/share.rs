use super::*;

#[test]
fn a_shared_folder_is_reported_only_if_it_did_not_map() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    scratch.write("program/guest/share-start.ps1", "map it at '@SHARE_ADDRESS@'");
    let agent = Arc::new(ScriptedAgent::answering(&["mapped\r\n", "not mapped\r\n", ""]));
    context.agent = agent.clone();
    let shared = Machine { share: "/folder".into(), ..machine(&context) };
    crate::agent::make_token(&context, &shared).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", shared.clone());
    apply_share(&mut task).unwrap();
    assert_eq!(task.error, "");
    apply_share(&mut task).unwrap();
    assert!(task.error.contains("did not reach"));
    task.error.clear();
    apply_share(&mut task).unwrap();
    assert!(task.error.contains("did not reach"));
    let asked = agent.requests.lock().unwrap().len();
    let key = std::fs::read_to_string(shared.share_key_file(&context)).unwrap().trim().to_owned();
    let sent: String = agent.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect();
    assert!(sent.contains(&format!("http://10.0.2.4/{key}/")), "the guest maps the share under its key");
    let mut alone = Task::new(&context, &host, "start", Machine { share: String::new(), ..shared });
    apply_share(&mut alone).unwrap();
    assert_eq!(agent.requests.lock().unwrap().len(), asked, "with no folder, the guest is not asked");
}
