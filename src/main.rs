// #[macro_use]
extern crate log;

use rust_discord::Bot;

#[tokio::main]
async fn main() {
    env_logger::init();
    let bot = Bot::new("config.json").expect("Failed to read config");

    bot.run().await
}
