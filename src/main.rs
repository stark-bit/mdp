mod config;
mod editor;
mod logger;
mod opener;
mod server;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    logger::init();
    std::panic::set_hook(Box::new(|panic| {
        error!("panic", "{panic}");
    }));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start tokio runtime");

    let config = config::Config::load_and_parse(std::env::args().skip(1));
    runtime.block_on(async {
        server::run(config).await;
    });
}
