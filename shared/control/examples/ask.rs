//! Asks the Guesthouse service one request, as the app would, and prints each answer as it comes:
//!
//!     cargo run -p guesthouse-control --example ask -- '{"ask":"status"}'
//!     cargo run -p guesthouse-control --example ask -- '{"ask":"watch","name":"ubuntu"}'
//!
//! For trying the service before the app, or beside it.

use guesthouse_control::Request;
use guesthouse_control::client::stream;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(text) = std::env::args().nth(1) else {
        eprintln!("Give a request as JSON, such as {{\"ask\":\"status\"}}.");
        return ExitCode::FAILURE;
    };
    let request: Request = match serde_json::from_str(&text) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("That isn't a request: {error}");
            return ExitCode::FAILURE;
        }
    };
    let asked = stream(&request, |answer| {
        println!("{}", serde_json::to_string(&answer).expect("an answer always serialises"));
        true
    });
    match asked {
        Ok(()) => ExitCode::SUCCESS,
        Err(unreached) => {
            eprintln!("{}", unreached.words());
            ExitCode::FAILURE
        }
    }
}
