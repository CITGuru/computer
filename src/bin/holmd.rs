fn main() -> Result<(), Box<dyn std::error::Error>> {
    holm_cli::adopt_earlier_names();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(holm_cli::daemon())
}
