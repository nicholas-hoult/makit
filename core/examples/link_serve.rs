//! Run the local session link on its own, for trying it without the app (#299):
//! `cargo run -p makit-core --example link_serve -- /tmp/makit-link-try/link.sock`
//! With no argument it uses the real path `~/.makit/link.sock`.
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from).or_else(makit_core::link::socket_path).expect("no home dir");
    let _link = makit_core::link::start_at(&path, Arc::new(makit_core::link::CoreBackend::new())).expect("bind");
    println!("link listening on {}", path.display());
    loop {
        std::thread::park();
    }
}
