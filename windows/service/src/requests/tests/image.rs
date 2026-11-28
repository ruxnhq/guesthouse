//! The image a machine is made from: opened as the user, and read by the service from the path that
//! open found, held until the machine is made.

use super::*;

/// A window whose user's open of a file finds it at `own`, as when the path asked for goes through a
/// link, and which keeps a count of how long what it held was held.
struct Resolving {
    own: PathBuf,
    held: Arc<()>,
    answers: Sender<Answer>,
}

impl Connection for Resolving {
    fn send(&mut self, answer: &Answer) -> io::Result<()> {
        self.answers.send(answer.clone()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn gone(&mut self) -> bool {
        false
    }

    fn may_read(&mut self, _: &Path) -> Result<(PathBuf, Held), String> {
        Ok((self.own.clone(), Box::new(Arc::clone(&self.held))))
    }

    fn may_share(&mut self, _: &Path, _: bool) -> Result<Held, String> {
        Ok(Box::new(()))
    }

    fn may_place(&mut self, _: &Path) -> Result<Held, String> {
        Ok(Box::new(()))
    }

    fn asker(&self) -> Asker {
        Asker { user: USER.into(), administrator: false }
    }
}

#[test]
fn the_image_is_read_where_the_users_own_open_found_it_and_held_until_the_machine_is_made() {
    let setup = setup("image-own", Pretend::default());
    let own = setup.root.join("own.iso");
    fs::write(&own, b"iso").unwrap();
    let mut machine = new_machine(&setup, "win");
    machine.image = setup.root.join("through-a-link").join("image.iso");
    let held = Arc::new(());
    let (answers, received) = channel();
    let mut window = Resolving { own: own.clone(), held: Arc::clone(&held), answers };
    setup.service.answer(Request::Create { machine }, &mut window);
    assert_eq!(received.try_iter().last(), Some(Answer::Done { reason: None }));
    assert_eq!(setup.service.work.prepared.lock().unwrap()[0].image, own);
    assert_eq!(Arc::strong_count(&held), 2, "the window's own, and none left from the making");
}
