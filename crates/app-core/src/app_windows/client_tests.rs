use super::message::*;
use super::stand_in::{FakeHelper, Welcome};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const SECONDS: Duration = Duration::from_secs(5);

/// Short waits, so that a test of a helper that says nothing does not take seconds.
fn quick() -> Timing {
    Timing {
        connect: Duration::from_millis(500),
        hello: Duration::from_millis(300),
        write: Duration::from_millis(500),
        reconnect_first: Duration::from_millis(20),
        reconnect_most: Duration::from_millis(100),
        start_cooldown: Duration::from_millis(400),
        request: Duration::from_millis(400),
        apps_request: Duration::from_millis(800),
    }
}

/// Counts its starts, says what it was made to, and can change the helper as a real start would.
struct Starter {
    starts: AtomicUsize,
    says: Mutex<Result<(), String>>,
    effect: Mutex<Option<Box<dyn FnMut() + Send>>>,
}

impl Starter {
    fn new() -> Arc<Self> {
        Arc::new(Self { starts: AtomicUsize::new(0), says: Mutex::new(Ok(())), effect: Mutex::new(None) })
    }

    fn count(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

impl HelperStarter for Starter {
    fn start(&self) -> Result<(), String> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if let Some(effect) = self.effect.lock().unwrap().as_mut() {
            effect();
        }
        self.says.lock().unwrap().clone()
    }
}

fn config(port: u16, starter: Option<&Arc<Starter>>) -> LinkConfig {
    LinkConfig {
        port,
        token: TOKEN.to_owned(),
        side: false,
        starter: starter.map(|starter| starter.clone() as Arc<dyn HelperStarter>),
        timing: quick(),
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn cell(id: &str, popup_of: Option<&str>) -> Cell {
    Cell {
        id: id.into(),
        pid: 1,
        exe: "x.exe".into(),
        app: String::new(),
        name: String::new(),
        cls: "X".into(),
        title: format!("window {id}"),
        cell: rect(0, 0, 300, 200),
        frame: rect(0, 0, 300, 200),
        popup: popup_of.is_some(),
        parent: popup_of.unwrap_or_default().into(),
        rel: Point { x: 4, y: 30 },
        state: State::Shown,
        overflow: false,
        focus: false,
        caption: Caption { h: 32, buttons_w: 138 },
        resizable: true,
        corner: 0,
    }
}

fn layout(seq: u64, cells: Vec<Cell>) -> Layout {
    Layout { seq, screen: Size { w: 1920, h: 1080 }, cells }
}

fn up(client: &mut Client) {
    assert!(client.wait_for(SECONDS, |c| *c.status() == Status::Up), "{:?}", client.status());
}

fn down_with(client: &mut Client, words: &str) {
    let found = client.wait_for(SECONDS, |c| matches!(c.status(), Status::Down(why) if why.contains(words)));
    assert!(found, "{:?}", client.status());
}

fn ids(client: &Client) -> Vec<String> {
    client.model().cells().iter().map(|cell| cell.id.clone()).collect()
}

#[test]
fn the_token_hello_and_layout_make_the_model_and_the_link_up() {
    let helper = FakeHelper::start(TOKEN);
    helper.with(|inner| inner.layout = layout(3, vec![cell("10", None), cell("11", Some("10"))]));
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    assert!(client.wait_for(SECONDS, |c| c.model().cells().len() == 2));
    assert_eq!(ids(&client), ["10", "11"]);
    assert_eq!(client.model().session(), Some("session-one"));
    assert_eq!(client.model().seq(), Some(3));
    assert!(client.last_layout_line().unwrap().contains(r#""t":"layout""#), "the line as it came");
    let updates = client.poll();
    assert!(
        updates.iter().any(|update| matches!(update, Update::Changes(changes) if changes.len() == 2)),
        "{updates:?}"
    );
}

#[test]
fn a_layout_the_helper_sends_later_comes_as_changes() {
    let helper = FakeHelper::start(TOKEN);
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    client.poll();
    helper.show(layout(2, vec![cell("10", None)]));
    assert!(client.wait_for(SECONDS, |c| c.model().cells().len() == 1));
    helper.show(layout(3, vec![cell("10", None), cell("12", Some("10"))]));
    assert!(client.wait_for(SECONDS, |c| c.model().cells().len() == 2));
    let changes: Vec<Change> = client
        .poll()
        .into_iter()
        .filter_map(|update| if let Update::Changes(changes) = update { Some(changes) } else { None })
        .flatten()
        .collect();
    assert!(
        matches!(changes.as_slice(), [Change::Added(first), Change::Added(second)] if first.id == "10" && second.popup),
        "{changes:?}"
    );
    helper.say_desktop("secure");
    assert!(client.wait_for(SECONDS, |c| c.model().desktop_now() == Desktop::Secure));
}

#[test]
fn a_request_is_answered_by_its_number() {
    let helper = FakeHelper::start(TOKEN);
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    let answer = client.ask(Request::Focus { id: "10".into() }, SECONDS).unwrap();
    assert_eq!((answer.re, answer.ok), (1, true));
    let answer = client.ask(Request::Minimize { id: "10".into() }, SECONDS).unwrap();
    assert_eq!(answer.re, 2);
    let seen = helper.requests();
    assert_eq!(
        (seen[0]["t"].as_str(), seen[0]["n"].as_u64(), seen[0]["id"].as_str()),
        (Some("focus"), Some(1), Some("10"))
    );
    assert_eq!(seen[1]["t"], "minimize");
}

#[test]
fn an_answer_that_says_no_has_the_helpers_words() {
    let helper = FakeHelper::start(TOKEN);
    helper.respond_with(|request, _| {
        vec![answer_line(request["n"].as_u64().unwrap(), false, "no such window")]
    });
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    let answer = client.ask(Request::Close { id: "9".into() }, SECONDS).unwrap();
    assert_eq!((answer.ok, answer.error.as_str()), (false, "no such window"));
}

#[test]
fn answers_come_back_to_the_requests_they_belong_to_whatever_the_order() {
    let helper = FakeHelper::start(TOKEN);
    let held: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    helper.respond_with(move |request, _| {
        let n = request["n"].as_u64().unwrap();
        let mut held = held.lock().unwrap();
        held.push(n);
        if held.len() < 2 {
            return Vec::new();
        }
        held.drain(..).rev().map(|n| answer_line(n, true, "")).collect()
    });
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    let first = client.request(Request::Restore { id: "1".into() });
    let second = client.request(Request::Close { id: "2".into() });
    let mut answered = Vec::new();
    while answered.len() < 2 {
        client.wait_for(Duration::from_millis(50), |_| false);
        for update in client.poll() {
            if let Update::Answered { n, result } = update {
                assert_eq!(result.unwrap().re, n);
                answered.push(n);
            }
        }
    }
    assert_eq!(answered, [second, first], "the second was answered first");
}

#[test]
fn a_request_nobody_answers_times_out_and_the_link_stays_up() {
    let helper = FakeHelper::start(TOKEN);
    helper.respond_with(|_, _| Vec::new());
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    assert_eq!(client.ask(Request::Layout, SECONDS), Err(RequestFailure::TimedOut));
    assert_eq!(*client.status(), Status::Up);
}

#[test]
fn a_request_while_down_is_refused_at_once_and_not_kept_for_later() {
    let helper = FakeHelper::start(TOKEN);
    helper.with(|inner| inner.welcome = Welcome::Close);
    let mut client = Client::start(config(helper.port, None));
    down_with(&mut client, "closed");
    assert_eq!(client.ask(Request::Focus { id: "1".into() }, SECONDS), Err(RequestFailure::NotConnected));
    helper.with(|inner| inner.welcome = Welcome::Hello);
    up(&mut client);
    assert!(helper.requests().is_empty(), "the focus was not sent when the link came back");
}

#[test]
fn a_dropped_connection_fails_what_waited_and_is_made_again_with_the_cells_kept() {
    let helper = FakeHelper::start(TOKEN);
    helper.with(|inner| inner.layout = layout(1, vec![cell("10", None)]));
    helper.respond_with(|_, _| Vec::new());
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    assert!(client.wait_for(SECONDS, |c| c.model().cells().len() == 1));
    let n = client.request(Request::Close { id: "10".into() });
    assert!(helper.wait_until(|inner| !inner.requests.is_empty()));
    helper.drop_connection();
    assert!(client.wait_for(SECONDS, |c| matches!(c.status(), Status::Down(_))));
    assert_eq!(ids(&client), ["10"], "a dropped connection does not close windows by itself");
    up(&mut client);
    assert_eq!(helper.connections(), 2);
    let lost = client.poll().into_iter().find_map(|update| match update {
        Update::Answered { n: of, result } if of == n => Some(result),
        _ => None,
    });
    assert_eq!(lost, Some(Err(RequestFailure::ConnectionLost)));
}

#[test]
fn a_helper_started_again_replaces_the_cells_and_the_old_handles_are_not_trusted() {
    let helper = FakeHelper::start(TOKEN);
    helper.with(|inner| inner.layout = layout(40, vec![cell("10", None)]));
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    assert!(client.wait_for(SECONDS, |c| c.model().seq() == Some(40)));
    client.poll();
    helper.with(|inner| {
        inner.session = "session-two".into();
        inner.layout = layout(1, vec![cell("10", None)]);
    });
    helper.drop_connection();
    assert!(
        client
            .wait_for(SECONDS, |c| c.model().session() == Some("session-two") && c.model().seq() == Some(1))
    );
    let changes: Vec<Change> = client
        .poll()
        .into_iter()
        .filter_map(|update| if let Update::Changes(changes) = update { Some(changes) } else { None })
        .flatten()
        .collect();
    let words: Vec<&str> = changes
        .iter()
        .map(|change| match change {
            Change::Restarted => "restarted",
            Change::Removed(_) => "removed",
            Change::Added(_) => "added",
            _ => "other",
        })
        .collect();
    assert_eq!(words, ["restarted", "removed", "added"]);
}

#[test]
fn a_wrong_token_looks_like_nothing_is_there_and_starts_the_helper_once_in_a_while() {
    let helper = FakeHelper::start("another token");
    let starter = Starter::new();
    let mut client = Client::start(config(helper.port, Some(&starter)));
    down_with(&mut client, "closed");
    assert!(helper.wait_until(|inner| inner.tokens_refused >= 2), "it tries again");
    assert_eq!(starter.count(), 1, "one start, not one a try");
    thread_sleep(500);
    assert!(starter.count() >= 2, "and again once the cooldown has gone");
    assert_eq!(helper.connections(), 0);
}

fn thread_sleep(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

#[test]
fn nothing_listening_starts_the_helper() {
    let port = {
        let spare = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        spare.local_addr().unwrap().port()
    };
    let starter = Starter::new();
    let mut client = Client::start(config(port, Some(&starter)));
    down_with(&mut client, "refused");
    let until = std::time::Instant::now() + SECONDS;
    while starter.count() == 0 && std::time::Instant::now() < until {
        thread_sleep(5);
    }
    assert_eq!(starter.count(), 1);
    assert!(client.model().cells().is_empty());
}

#[test]
fn a_helper_that_takes_the_token_and_says_nothing_is_given_up_on_after_the_hello_time() {
    let helper = FakeHelper::start(TOKEN);
    helper.with(|inner| inner.welcome = Welcome::Silent);
    let starter = Starter::new();
    let started = starter.clone();
    let for_effect = helper_welcome(&helper);
    *starter.effect.lock().unwrap() = Some(Box::new(for_effect));
    let mut client = Client::start(config(helper.port, Some(&starter)));
    down_with(&mut client, "did not say hello");
    up(&mut client);
    assert_eq!(started.count(), 1, "the start mended it, and nothing started it again");
}

/// What a start does to the pretend helper: it says hello from now on.
fn helper_welcome(helper: &FakeHelper) -> impl Fn() + Send + 'static {
    let inner = helper.inner_for_test();
    move || inner.lock().unwrap().welcome = Welcome::Hello
}

#[test]
fn a_start_that_fails_is_the_reason_the_link_is_down() {
    let helper = FakeHelper::start("another token");
    let starter = Starter::new();
    *starter.says.lock().unwrap() = Err("Restart the machine so that it has the forward.".into());
    let mut client = Client::start(config(helper.port, Some(&starter)));
    down_with(&mut client, "Restart the machine");
    assert!(
        client.poll().iter().any(|update| matches!(update, Update::Note(text) if text.contains("Restart")))
    );
}

#[test]
fn a_helper_of_another_version_is_restarted_once_and_then_left_alone() {
    let helper = FakeHelper::start(TOKEN);
    helper.with(|inner| inner.version = 2);
    let starter = Starter::new();
    let mut client = Client::start(config(helper.port, Some(&starter)));
    down_with(&mut client, "version 2");
    thread_sleep(900);
    assert_eq!(starter.count(), 1, "a second restart would find the same");
    assert!(client.model().session().is_none());
}

#[test]
fn a_line_over_the_limit_is_passed_over_and_the_link_goes_on() {
    let helper = FakeHelper::start(TOKEN);
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    client.poll();
    helper.say(&format!(r#"{{"t":"desktop","kind":"normal","pad":"{}"}}"#, "x".repeat(MOST_LINE_BYTES + 10)));
    helper.show(layout(2, vec![cell("10", None)]));
    assert!(client.wait_for(SECONDS, |c| c.model().cells().len() == 1));
    assert!(
        client.poll().iter().any(|update| matches!(update, Update::Note(text) if text.contains("a line of")))
    );
}

#[test]
fn a_line_that_is_not_a_message_is_noted_and_the_link_goes_on() {
    let helper = FakeHelper::start(TOKEN);
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    client.poll();
    helper.say("this is not json");
    helper.say(r#"{"t":"layout","seq":"x"}"#);
    helper.show(layout(2, vec![cell("10", None)]));
    assert!(client.wait_for(SECONDS, |c| c.model().cells().len() == 1));
    let notes = client.poll().into_iter().filter(|update| matches!(update, Update::Note(_))).count();
    assert_eq!(notes, 2);
}

#[test]
fn an_answer_to_apps_may_be_longer_than_other_lines() {
    let helper = FakeHelper::start(TOKEN);
    helper.respond_with(|request, _| {
        let n = request["n"].as_u64().unwrap();
        let icon = "A".repeat(MOST_LINE_BYTES + 100);
        vec![format!(
            r#"{{"t":"answer","re":{n},"ok":true,"apps":[{{"id":"a","name":"A","icon":"{icon}"}}]}}"#
        )]
    });
    let mut client = Client::start(config(helper.port, None));
    up(&mut client);
    let answer = client.ask(Request::Apps { icons: true }, SECONDS).unwrap();
    assert_eq!(answer.apps.unwrap()[0].icon.len(), MOST_LINE_BYTES + 100);
}

#[test]
fn a_second_client_takes_the_connection_and_the_first_comes_back_later() {
    let helper = FakeHelper::start(TOKEN);
    let mut first = Client::start(config(helper.port, None));
    up(&mut first);
    let mut second = Client::start(config(helper.port, None));
    up(&mut second);
    assert!(first.wait_for(SECONDS, |c| matches!(c.status(), Status::Down(_))), "the first was replaced");
    up(&mut first);
    assert!(helper.connections() >= 3, "and it took the connection back after its wait");
}

#[test]
fn a_side_client_works_beside_the_driving_one_and_does_not_disturb_it() {
    let helper = FakeHelper::start(TOKEN);
    let mut driver = Client::start(config(helper.port, None));
    up(&mut driver);
    let mut side = Client::start(LinkConfig { side: true, ..config(helper.port, None) });
    up(&mut side);
    let answer = side.ask(Request::Layout, SECONDS).unwrap();
    assert!(answer.ok);
    // The driving connection was never replaced, so it stayed up and made no second connection.
    assert_eq!(*driver.status(), Status::Up);
    assert_eq!((helper.connections(), helper.with(|inner| inner.side_connections)), (1, 1));
    let answer = driver.ask(Request::Focus { id: "10".into() }, SECONDS).unwrap();
    assert!(answer.ok, "and it still drives");
}

#[test]
fn two_driving_clients_still_take_the_connection_from_each_other() {
    let helper = FakeHelper::start(TOKEN);
    let mut first = Client::start(config(helper.port, None));
    up(&mut first);
    let mut side = Client::start(LinkConfig { side: true, ..config(helper.port, None) });
    up(&mut side);
    let mut second = Client::start(config(helper.port, None));
    up(&mut second);
    assert!(
        first.wait_for(SECONDS, |c| matches!(c.status(), Status::Down(_))),
        "a second driver replaces the first"
    );
    assert_eq!(*side.status(), Status::Up, "and a side one is not touched by either");
}
