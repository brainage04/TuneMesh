//! The worker's command line and size options never panic. One argument
//! per input line (so `dicts/worker_args.dict` can supply option names).
#![no_main]

use libfuzzer_sys::fuzz_target;
use tunemesh_worker::config::{Parsed, parse_args, parse_size};

fuzz_target!(|s: &str| {
    let args: Vec<String> = s.split('\n').map(str::to_owned).collect();
    for a in &args {
        let _ = parse_size(a);
    }
    if let Ok(Parsed::Run(a)) = parse_args(args) {
        assert!(a.limits.max_items > 0 && a.limits.max_pending > 0);
    }
});
