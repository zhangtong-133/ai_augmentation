#![forbid(unsafe_code)]

use std::env;
use std::thread;
use std::time::Duration;

fn main() {
    println!("worker bootstrap complete; adapters are not configured yet");
    wait_if_supervised();
}

fn wait_if_supervised() {
    if env::var("RUN_FOREVER").as_deref() == Ok("1") {
        loop {
            thread::park_timeout(Duration::from_mins(1));
        }
    }
}
