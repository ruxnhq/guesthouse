//! What a drag out of the guest's screen offers other programs: its files, as soon as they are
//! here. A program that asks before then waits for them.

use gtk::gdk::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;

/// How file managers and browsers take files from a drag.
const URI_LIST: &str = "text/uri-list";

/// The files of one drag, once they have been fetched, or why they weren't.
pub struct Landing {
    fetched: RefCell<Option<Result<Vec<PathBuf>, String>>>,
    /// Closed, which wakes everyone waiting, once the files are here.
    arrival: RefCell<Option<async_channel::Sender<()>>>,
    waiting: async_channel::Receiver<()>,
}

impl Landing {
    fn new() -> Rc<Self> {
        let (arrival, waiting) = async_channel::bounded(1);
        Rc::new(Self { fetched: RefCell::default(), arrival: RefCell::new(Some(arrival)), waiting })
    }

    pub(super) fn finish(&self, fetched: Result<Vec<PathBuf>, String>) {
        *self.fetched.borrow_mut() = Some(fetched);
        self.arrival.borrow_mut().take();
    }

    async fn uri_list(&self) -> Result<String, glib::Error> {
        if self.fetched.borrow().is_none() {
            let _ = self.waiting.recv().await;
        }
        match self.fetched.borrow().as_ref() {
            Some(Ok(paths)) => {
                Ok(paths.iter().map(|path| format!("{}\r\n", gio::File::for_path(path).uri())).collect())
            }
            Some(Err(text)) => Err(glib::Error::new(gio::IOErrorEnum::Failed, text)),
            None => Err(glib::Error::new(gio::IOErrorEnum::Cancelled, "the drag ended")),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct DraggedFiles {
        pub(super) landing: RefCell<Option<Rc<Landing>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DraggedFiles {
        const NAME: &'static str = "GuesthouseDraggedFiles";
        type Type = super::DraggedFiles;
        type ParentType = gdk::ContentProvider;
    }

    impl ObjectImpl for DraggedFiles {}

    impl ContentProviderImpl for DraggedFiles {
        fn formats(&self) -> gdk::ContentFormats {
            gdk::ContentFormats::new(&[URI_LIST])
        }

        fn write_mime_type_future(
            &self,
            _mime_type: &str,
            stream: &gio::OutputStream,
            priority: glib::Priority,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>> + 'static>> {
            let (landing, stream) = (self.landing.borrow().clone(), stream.clone());
            Box::pin(async move {
                let Some(landing) = landing else { return Ok(()) };
                let list = landing.uri_list().await?;
                stream.write_all_future(list.into_bytes(), priority).await.map_err(|(_, error)| error)?;
                Ok(())
            })
        }
    }
}

glib::wrapper! {
    pub struct DraggedFiles(ObjectSubclass<imp::DraggedFiles>) @extends gdk::ContentProvider;
}

impl DraggedFiles {
    /// The provider for a drag, and where to say that its files have landed.
    pub(super) fn new() -> (Self, Rc<Landing>) {
        let provider: Self = glib::Object::new();
        let landing = Landing::new();
        *provider.imp().landing.borrow_mut() = Some(landing.clone());
        (provider, landing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_asks_early_gets_the_files_once_they_have_landed() {
        let landing = Landing::new();
        let context = glib::MainContext::new();
        let asking = landing.clone();
        let asked = context.spawn_local(async move { asking.uri_list().await });
        context.iteration(false);
        landing.finish(Ok(vec![PathBuf::from("/tmp/a b.txt"), PathBuf::from("/tmp/trip")]));
        let list = context.block_on(asked).unwrap().unwrap();
        assert_eq!(list, "file:///tmp/a%20b.txt\r\nfile:///tmp/trip\r\n");
    }

    #[test]
    fn files_that_did_not_arrive_are_an_error_for_whoever_asks() {
        let landing = Landing::new();
        landing.finish(Err("the guest went away".to_owned()));
        let asked = glib::MainContext::new().block_on(landing.uri_list());
        assert!(asked.unwrap_err().to_string().contains("the guest went away"));
    }
}
