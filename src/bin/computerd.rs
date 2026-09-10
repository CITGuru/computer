//! The `computerd` daemon; what it does lives in `computer-cli`.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    computer_cli::daemon().await
}
