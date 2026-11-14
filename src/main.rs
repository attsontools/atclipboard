use atclipboard_service::{
    parse_service_args, run_service_process, run_x11_clipboard_owner_if_requested,
};

fn main() {
    if run_x11_clipboard_owner_if_requested() {
        return;
    }
    let result = parse_service_args(std::env::args_os().skip(1))
        .and_then(|options| run_service_process(options, env!("CARGO_PKG_VERSION")));
    if let Err(error) = result {
        eprintln!("{}: {}", error.code, error.message);
        std::process::exit(1);
    }
}
