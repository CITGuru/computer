//! The `computer` command; what it does lives in `computer-cli`.

#[tokio::main]
async fn main() {
    if let Err(error) = computer_cli::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
