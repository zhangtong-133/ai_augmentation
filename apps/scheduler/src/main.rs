#![forbid(unsafe_code)]

use std::env;
use std::thread;
use std::time::Duration;

fn main() {
    println!("scheduler bootstrap complete; no schedules are registered yet");
    wait_if_supervised();
}

fn wait_if_supervised() {
    if env::var("RUN_FOREVER").as_deref() == Ok("1") {
        loop {
            thread::park_timeout(Duration::from_mins(1));
        }
    }
}
