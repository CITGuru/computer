#[tokio::main]
async fn main() {
    if let Err(error) = computer_cli::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
