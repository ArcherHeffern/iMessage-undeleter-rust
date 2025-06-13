#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
mod app;
mod exporters;

pub use exporters::txt::TXT;

use app::{
    options::{Options, from_command_line},
    runtime::Config,
};

fn main() {
    // Get args from command line
    let args = from_command_line();
    // Create application options
    let options = Options::from_args(&args);

    // Create app state and start
    match options {
        Err(why) => {
            eprintln!("{why}");
        },
        Ok(options) => match Config::new(options) {
            Ok(mut app) => {
                // Resolve the filtered contacts, if provided
                app.resolve_filtered_handles();

                if let Err(why) = app.start() {
                    eprintln!("Unable to start: {why}");
                }
            }
            Err(why) => {
                eprintln!("Invalid configuration: {why}");
            }
        }
    }
}
