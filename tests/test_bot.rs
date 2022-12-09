// #[macro_use]
extern crate log;

use rust_discord::Bot;

#[test]
fn it_can_connect() {
    env_logger::init();
    let _bot = Bot::new("config.json");
}
